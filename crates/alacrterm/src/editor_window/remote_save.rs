//! Optimistic conflict detection before remote write-back.
use super::text_document::RemoteVersion;
use futures::StreamExt as _;
use terminal::{SshFs, TransferEvent};

pub(super) async fn remote_matches(
    fs: SshFs,
    path: String,
    expected: RemoteVersion,
) -> Result<bool, String> {
    let metadata = fs
        .stat(path.clone())
        .await
        .map_err(|error| format!("无法检查远端文件：{error}"))?;
    if metadata.is_dir || metadata.size != expected.size {
        return Ok(false);
    }
    // Content comparison also catches same-size edits within coarse mtime granularity.
    let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
    let snapshot = directory.path().join("remote-snapshot");
    let mut events = fs.download_one(path, snapshot.clone()).events;
    while let Some(event) = events.next().await {
        if let TransferEvent::Done(result) = event {
            result.map_err(|error| format!("无法读取远端校验副本：{error}"))?;
            let bytes = std::fs::read(snapshot).map_err(|error| error.to_string())?;
            return Ok(RemoteVersion::of(&bytes) == expected);
        }
    }
    Err("远端校验传输中断".into())
}

#[cfg(all(test, unix))]
#[path = "../../../ssh/examples/local_server.rs"]
mod local_server;

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sftp_conflict_check_detects_same_size_edits_and_never_overwrites_them() {
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("remote.txt");
            std::fs::write(&path, b"before").unwrap();
            let port = local_server::serve().await.unwrap();
            let mut params = ssh::SshParams::new("127.0.0.1", port, "tester")
                .with_password(local_server::PASSWORD);
            params.host_key = ssh::HostKeyPolicy::AcceptNew;
            params.known_hosts = Some(directory.path().join("known_hosts"));
            let source = SshFs::new(params);
            let remote = path.to_str().unwrap().to_string();
            let expected = RemoteVersion::of(b"before");
            assert!(
                remote_matches(source.clone(), remote.clone(), expected)
                    .await
                    .unwrap()
            );
            std::fs::write(&path, b"after!").unwrap();
            assert!(
                !remote_matches(source.clone(), remote.clone(), expected)
                    .await
                    .unwrap()
            );
            assert_eq!(std::fs::read(&path).unwrap(), b"after!");
            std::fs::write(&path, b"different size").unwrap();
            assert!(
                !remote_matches(source.clone(), remote.clone(), expected)
                    .await
                    .unwrap()
            );
            std::fs::remove_file(&path).unwrap();
            assert!(remote_matches(source, remote, expected).await.is_err());
        })
        .await
        .expect("local SFTP conflict check timed out");
    }
}
