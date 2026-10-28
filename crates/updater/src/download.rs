use crate::manifest::{self, Asset, Manifest, PackageKind, Release};
use anyhow::{Context as _, Result, ensure};
use reqwest::{Client, StatusCode, header::RANGE};
use sha2::{Digest as _, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

pub struct Feed {
    client: Client,
    runtime: tokio::runtime::Runtime,
}

impl Feed {
    pub fn new() -> Result<Self> {
        Ok(Self {
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?,
            client: Client::builder()
                .user_agent(concat!("Alacrterm/", env!("CARGO_PKG_VERSION")))
                .https_only(true)
                .connect_timeout(Duration::from_secs(30))
                .timeout(Duration::from_secs(30 * 60))
                .build()?,
        })
    }

    pub fn check(&self, url: &str, keys: &[String]) -> Result<Manifest> {
        self.check_signed(url, keys).map(|(manifest, _)| manifest)
    }

    pub fn check_signed(&self, url: &str, keys: &[String]) -> Result<(Manifest, Vec<u8>)> {
        self.runtime.block_on(async {
            let mut response = self.client.get(url).send().await?.error_for_status()?;
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                ensure!(bytes.len() + chunk.len() <= 1024 * 1024, "更新清单过大");
                bytes.extend_from_slice(&chunk);
            }
            Ok((manifest::open(&bytes, keys)?, bytes))
        })
    }

    pub fn download(
        &self,
        release: &Release,
        folder: &Path,
        cancel: &AtomicBool,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<PathBuf> {
        self.runtime
            .block_on(cancellable(cancel, Duration::from_secs(30 * 60), async {
                self.download_inner(release, folder, cancel, &mut progress)
                    .await
            }))
    }

    async fn download_inner(
        &self,
        release: &Release,
        folder: &Path,
        cancel: &AtomicBool,
        progress: &mut impl FnMut(u64, u64),
    ) -> Result<PathBuf> {
        ensure!(!cancel.load(Ordering::Relaxed), "下载已取消");
        ensure!(
            release.asset.size > 0
                && release.asset.sha256.len() == 64
                && release
                    .asset
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit()),
            "安装包元数据无效"
        );
        fs::create_dir_all(folder)?;
        let extension = match release.asset.kind {
            PackageKind::Exe => "exe",
            PackageKind::Deb => "deb",
        };
        let name = format!(
            "alacrterm-{}-{}.{}",
            release.version,
            &release.asset.sha256[..16],
            extension
        );
        let destination = folder.join(name);
        if verify_cancellable(&destination, &release.asset, cancel).is_ok() {
            return Ok(destination);
        }
        let part = destination.with_extension(format!("{extension}.part"));
        let mut have = fs::metadata(&part).map_or(0, |metadata| metadata.len());
        if have > release.asset.size {
            fs::remove_file(&part)?;
            have = 0;
        }
        if have < release.asset.size {
            let mut request = self.client.get(&release.asset.url);
            if have > 0 {
                request = request.header(RANGE, format!("bytes={have}-"));
            }
            let mut response = cancellable(cancel, Duration::from_secs(30), async {
                Ok(request.send().await?.error_for_status()?)
            })
            .await?;
            let append = response.status() == StatusCode::PARTIAL_CONTENT && have > 0;
            if append {
                let range = response
                    .headers()
                    .get("content-range")
                    .and_then(|value| value.to_str().ok())
                    .context("续传响应缺少 Content-Range")?;
                ensure!(
                    range.starts_with(&format!("bytes {have}-")),
                    "续传起点不匹配"
                );
            } else {
                ensure!(response.status() == StatusCode::OK, "下载响应无效");
                have = 0;
            }
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .append(append)
                .truncate(!append)
                .open(&part)?;
            let mut percent = 101;
            loop {
                ensure!(!cancel.load(Ordering::Relaxed), "下载已取消");
                let Some(chunk) = cancellable(cancel, Duration::from_secs(30), async {
                    Ok(response.chunk().await?)
                })
                .await?
                else {
                    break;
                };
                let count = chunk.len();
                have += count as u64;
                ensure!(have <= release.asset.size, "下载超出清单声明大小");
                file.write_all(&chunk)?;
                let next = have.saturating_mul(100) / release.asset.size;
                if next != percent {
                    progress(have, release.asset.size);
                    percent = next;
                }
            }
            file.sync_all()?;
        }
        ensure!(!cancel.load(Ordering::Relaxed), "下载已取消");
        if let Err(error) = verify_cancellable(&part, &release.asset, cancel) {
            // Incomplete files remain resumable; a full but corrupt package must be redownloaded.
            if have == release.asset.size {
                fs::remove_file(&part)?;
            }
            return Err(error);
        }
        fs::rename(&part, &destination)?;
        Ok(destination)
    }
}

pub fn verify(path: &Path, asset: &Asset) -> Result<()> {
    verify_cancellable(path, asset, &AtomicBool::new(false))
}

fn verify_cancellable(path: &Path, asset: &Asset, cancel: &AtomicBool) -> Result<()> {
    ensure!(fs::metadata(path)?.len() == asset.size, "安装包大小不匹配");
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        ensure!(!cancel.load(Ordering::Relaxed), "下载已取消");
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let digest = format!("{:x}", hasher.finalize());
    ensure!(
        digest.eq_ignore_ascii_case(&asset.sha256),
        "安装包 SHA-256 校验失败"
    );
    Ok(())
}

/// Dropping a request/body future closes the connection, including stalled reads.
async fn cancellable<T>(
    cancel: &AtomicBool,
    timeout: Duration,
    future: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    let cancellation = async {
        loop {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    };
    tokio::select! {
        biased;
        _ = cancellation => anyhow::bail!("下载已取消"),
        result = tokio::time::timeout(timeout, future) => result.context("下载等待超时")?,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_interrupts_stalled_headers_and_body_and_preserves_partial_file() {
        use std::{
            net::TcpListener,
            sync::{Arc, mpsc},
            time::Instant,
        };
        for send_headers in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let (started, ready) = mpsc::channel();
            let (finish, done) = mpsc::channel();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 4096];
                assert!(stream.read(&mut request).unwrap() > 0);
                if send_headers {
                    stream
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nab")
                        .unwrap();
                }
                started.send(()).unwrap();
                done.recv_timeout(Duration::from_secs(5)).unwrap();
            });
            let temp = tempfile::tempdir().unwrap();
            let cancel = Arc::new(AtomicBool::new(false));
            let cancelling = cancel.clone();
            let worker = std::thread::spawn(move || {
                ready.recv().unwrap();
                cancelling.store(true, Ordering::Relaxed);
            });
            let feed = Feed {
                client: Client::builder().no_proxy().build().unwrap(),
                runtime: tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap(),
            };
            let release = Release {
                version: semver::Version::new(0, 2, 0),
                notes: String::new(),
                asset: Asset {
                    url: format!("http://{address}/app.deb"),
                    size: 6,
                    sha256: format!("{:x}", Sha256::digest(b"abcdef")),
                    kind: PackageKind::Deb,
                },
            };
            let start = Instant::now();
            let result = feed.download(&release, temp.path(), &cancel, |_, _| {});
            assert!(result.unwrap_err().to_string().contains("取消"));
            assert!(start.elapsed() < Duration::from_secs(2));
            finish.send(()).unwrap();
            worker.join().unwrap();
            server.join().unwrap();
        }
    }

    #[test]
    fn interrupted_download_resumes_and_validated_cache_avoids_network() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let count = stream.read(&mut request).unwrap();
            assert!(
                String::from_utf8_lossy(&request[..count])
                    .to_lowercase()
                    .contains("range: bytes=2-")
            );
            stream.write_all(b"HTTP/1.1 206 Partial Content\r\nContent-Length: 4\r\nContent-Range: bytes 2-5/6\r\nConnection: close\r\n\r\ncdef").unwrap();
        });
        let temp = tempfile::tempdir().unwrap();
        let digest = format!("{:x}", Sha256::digest(b"abcdef"));
        let part = temp
            .path()
            .join(format!("alacrterm-0.2.0-{}.deb.part", &digest[..16]));
        fs::write(&part, b"ab").unwrap();
        let release = Release {
            version: semver::Version::new(0, 2, 0),
            notes: String::new(),
            asset: Asset {
                url: format!("http://{address}/app.deb"),
                size: 6,
                sha256: digest,
                kind: PackageKind::Deb,
            },
        };
        let feed = Feed {
            client: Client::builder().no_proxy().build().unwrap(),
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        };
        let cancel = AtomicBool::new(false);
        let path = feed
            .download(&release, temp.path(), &cancel, |_, _| {})
            .unwrap();
        server.join().unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"abcdef");
        assert!(!part.exists());
        assert_eq!(
            feed.download(&release, temp.path(), &cancel, |_, _| {})
                .unwrap(),
            path
        );
    }

    #[test]
    fn package_integrity_detects_corruption_and_truncation() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("package.deb");
        fs::write(&path, b"abc").unwrap();
        let asset = Asset {
            url: "https://example.com/app.deb".into(),
            size: 3,
            sha256: format!("{:x}", Sha256::digest(b"abc")),
            kind: PackageKind::Deb,
        };
        assert!(verify(&path, &asset).is_ok());
        fs::write(&path, b"abd").unwrap();
        assert!(verify(&path, &asset).is_err());
        fs::write(&path, b"ab").unwrap();
        assert!(verify(&path, &asset).is_err());
    }
}
