use super::terminal::Host;
use agent::{
    tools::{Cancel, DirectoryEntry, DirectoryListing, FileContent, FilesystemHost},
    types::TargetBinding,
};
use async_trait::async_trait;
use futures::StreamExt as _;
use sha2::{Digest as _, Sha256};
use std::{
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub(super) enum Filesystem {
    Local(PathBuf),
    Remote {
        fs: ssh::SshFs,
        root: Option<String>,
    },
}
fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn text(bytes: Vec<u8>, path: String) -> Result<FileContent, String> {
    if bytes.len() > 65536 || bytes.contains(&0) {
        return Err("file is not text or exceeds 64 KiB".into());
    }
    let sha256 = digest(&bytes);
    let content = String::from_utf8(bytes).map_err(|_| "file must contain UTF-8 text")?;
    Ok(FileContent {
        path,
        content,
        sha256,
    })
}
fn resolve(root: &Path, path: &str) -> Result<PathBuf, String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let path = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        root.join(path)
    };
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    if !path.starts_with(&root) {
        return Err("path is outside the bound directory".into());
    }
    Ok(path)
}
fn local_read(root: &Path, path: &str) -> Result<FileContent, String> {
    let path = resolve(root, path)?;
    if !path.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("path is not a regular file".into());
    }
    let mut bytes = vec![];
    std::fs::File::open(&path)
        .map_err(|e| e.to_string())?
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    text(bytes, path.to_string_lossy().into_owned())
}
fn local_list(root: &Path, path: &str) -> Result<DirectoryListing, String> {
    let path = resolve(root, path)?;
    let mut entries = vec![];
    for entry in std::fs::read_dir(path)
        .map_err(|e| e.to_string())?
        .take(201)
    {
        let entry = entry.map_err(|e| e.to_string())?;
        let metadata = entry.metadata().map_err(|e| e.to_string())?;
        entries.push(DirectoryEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            is_dir: metadata.is_dir(),
            size: metadata.len(),
        });
    }
    Ok(listing(entries))
}
fn listing(mut entries: Vec<DirectoryEntry>) -> DirectoryListing {
    let truncated = entries.len() > 200;
    entries.truncate(200);
    entries.sort_by_key(|e| (!e.is_dir, e.name.to_lowercase()));
    DirectoryListing { entries, truncated }
}
fn local_write(
    root: &Path,
    path: &str,
    content: &str,
    expected: &str,
    cancel: &Cancel,
) -> Result<(), String> {
    let current = local_read(root, path)?;
    if current.sha256 != expected {
        return Err("file changed since it was read; read it again before editing".into());
    }
    if cancel.is_cancelled() {
        return Err("operation cancelled".into());
    }
    util::atomic_file::write(Path::new(&current.path), content.as_bytes())
        .map_err(|e| e.to_string())
}

async fn remote_path(fs: &ssh::SshFs, root: &Option<String>, path: &str) -> Result<String, String> {
    let home = fs.home_dir().await.map_err(|e| e.to_string())?;
    let root = match root.as_deref() {
        Some("~") | None => home.clone(),
        Some(root) if root.starts_with("~/") => {
            format!("{}/{}", home.trim_end_matches('/'), &root[2..])
        }
        Some(root) => root.to_owned(),
    };
    let root = fs.canonicalize(root).await.map_err(|e| e.to_string())?;
    let path = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("{}/{}", root.trim_end_matches('/'), path)
    };
    let path = fs.canonicalize(path).await.map_err(|e| e.to_string())?;
    if root != "/" && path != root && !path.starts_with(&format!("{}/", root.trim_end_matches('/')))
    {
        return Err("path is outside the bound remote directory".into());
    }
    Ok(path)
}
async fn remote_read(
    fs: &ssh::SshFs,
    root: &Option<String>,
    path: &str,
) -> Result<FileContent, String> {
    let path = remote_path(fs, root, path).await?;
    let metadata = fs.stat(path.clone()).await.map_err(|e| e.to_string())?;
    if metadata.is_dir || metadata.size > 65536 {
        return Err("path is a directory or file exceeds 64 KiB".into());
    }
    text(
        fs.read_completion_file(path.clone())
            .await
            .map_err(|e| e.to_string())?,
        path,
    )
}
struct TransferGuard(ssh::CancelToken);
impl Drop for TransferGuard {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

#[async_trait]
impl FilesystemHost for Host {
    async fn read(&self, target: &TargetBinding, path: &str) -> Result<FileContent, String> {
        self.validate(target).await?;
        match self
            .files
            .as_ref()
            .ok_or("terminal directory unavailable")?
        {
            Filesystem::Local(root) => {
                let root = root.clone();
                let path = path.to_owned();
                tokio::task::spawn_blocking(move || local_read(&root, &path))
                    .await
                    .map_err(|e| e.to_string())?
            }
            Filesystem::Remote { fs, root } => tokio::time::timeout(
                std::time::Duration::from_secs(30),
                remote_read(fs, root, path),
            )
            .await
            .map_err(|_| "remote file read timed out".to_owned())?,
        }
    }
    async fn list(&self, target: &TargetBinding, path: &str) -> Result<DirectoryListing, String> {
        self.validate(target).await?;
        match self
            .files
            .as_ref()
            .ok_or("terminal directory unavailable")?
        {
            Filesystem::Local(root) => {
                let root = root.clone();
                let path = path.to_owned();
                tokio::task::spawn_blocking(move || local_list(&root, &path))
                    .await
                    .map_err(|e| e.to_string())?
            }
            Filesystem::Remote { fs, root } => {
                let operation = async {
                    let path = remote_path(fs, root, path).await?;
                    let entries = fs
                        .list_completion_dir(path)
                        .await
                        .map_err(|e| e.to_string())?;
                    Ok(listing(
                        entries
                            .into_iter()
                            .map(|e| DirectoryEntry {
                                name: e.name,
                                is_dir: e.is_dir,
                                size: e.size,
                            })
                            .collect(),
                    ))
                };
                tokio::time::timeout(std::time::Duration::from_secs(30), operation)
                    .await
                    .map_err(|_| "remote directory read timed out".to_owned())?
            }
        }
    }
    async fn write(
        &self,
        target: &TargetBinding,
        path: &str,
        content: &str,
        expected: &str,
        cancel: Cancel,
    ) -> Result<(), String> {
        self.validate(target).await?;
        match self
            .files
            .as_ref()
            .ok_or("terminal directory unavailable")?
        {
            Filesystem::Local(root) => {
                let root = root.clone();
                let path = path.to_owned();
                let content = content.to_owned();
                let expected = expected.to_owned();
                tokio::task::spawn_blocking(move || {
                    local_write(&root, &path, &content, &expected, &cancel)
                })
                .await
                .map_err(|e| e.to_string())?
            }
            Filesystem::Remote { fs, root } => {
                let operation = async {
                    let current = remote_read(fs, root, path).await?;
                    if current.sha256 != expected {
                        return Err(
                            "remote file changed since it was read; read it again before editing"
                                .into(),
                        );
                    }
                    if cancel.is_cancelled() {
                        return Err("operation cancelled".into());
                    }
                    let mut file = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
                    file.write_all(content.as_bytes())
                        .map_err(|e| e.to_string())?;
                    file.flush().map_err(|e| e.to_string())?;
                    // UploadOne uses the existing transfer worker's atomic
                    // temporary-file/rename commit, never an in-place truncate.
                    let transfer = fs.upload_one_checked(
                        file.path(),
                        current.path,
                        current.content.into_bytes(),
                    );
                    let _guard = TransferGuard(transfer.cancel().clone());
                    let mut events = transfer.events;
                    while let Some(event) = events.next().await {
                        if let ssh::TransferEvent::Done(result) = event {
                            return result.map_err(|e| e.to_string());
                        }
                    }
                    Err("remote write ended without a completion result".into())
                };
                tokio::time::timeout(std::time::Duration::from_secs(30), operation)
                    .await
                    .map_err(|_| {
                        "remote file write timed out; inspect the file before retrying".to_owned()
                    })?
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_file_edits_require_current_hash_and_preserve_original_on_conflict() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let path = root.join("file.txt");
        std::fs::write(&path, "before").unwrap();
        let file = local_read(root, "file.txt").unwrap();
        std::fs::write(&path, "after!").unwrap();
        assert!(local_write(root, "file.txt", "new", &file.sha256, &Cancel::new()).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "after!");
        let file = local_read(root, "file.txt").unwrap();
        local_write(root, "file.txt", "new", &file.sha256, &Cancel::new()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        assert!(local_write(root, "new.txt", "new", &file.sha256, &Cancel::new()).is_err());
    }
    #[test]
    fn file_tools_reject_binary_oversized_and_outside_paths() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("workspace");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(directory.path().join("outside"), "private").unwrap();
        assert!(local_read(&root, "../outside").is_err());
        std::fs::write(root.join("binary"), b"a\0b").unwrap();
        assert!(local_read(&root, "binary").is_err());
        std::fs::write(root.join("large"), vec![b'x'; 65537]).unwrap();
        assert!(local_read(&root, "large").is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(directory.path().join("outside"), root.join("link"))
                .unwrap();
            assert!(local_read(&root, "link").is_err());
        }
    }
}
