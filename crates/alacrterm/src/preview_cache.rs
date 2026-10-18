//! 预览缓存：**每个进程一个实例目录** + 可共享的缓存租约。
//!
//! 旧做法是「所有进程共用一个 `preview/` 目录 + 启动时整个删掉」，两个实例同时跑时
//! 后启动的那个会把前一个正在读 / 正在播的文件删掉。现在改成：
//!
//! - 根目录下按 `<pid>-<启动时间>` 建**实例目录**，每个进程只碰自己的；
//! - 实例目录里放一个 `.lock` 文件并在进程存活期间持有文件锁 —— 别的实例清理时
//!   `try_lock` 会失败，于是跳过这个目录；
//! - 启动时**只清理拿得到锁**的旧实例目录（上次崩溃 / 正常退出留下的）。
//!
//! 文件的生命周期由 [`CacheLease`] 管：`Clone` 是共享同一份文件，**最后一个**句柄析构时
//! 删除。下载、预览标签、保存流程各持一份，谁先结束都不会提前删掉别人还要用的文件。
//!
//! 交给系统播放器的副本（音频 / 视频）用 [`PreviewCache::allocate_detached`]：不挂租约，
//! 应用退出时不强删（播放器可能还在读），留给下一次启动的实例清理。

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{App, Global};

/// 实例目录里的锁文件。
const LOCK_FILE: &str = ".lock";

/// 预览缓存（全局唯一，随进程启动安装一次）。
pub(crate) struct PreviewCache {
    /// 本进程的实例目录。
    instance: PathBuf,
    /// 实例锁：活到进程结束，期间别的实例不会清理本目录。
    _lock: Option<File>,
    /// 文件名里的递增序号（同一实例内不重名）。
    next: AtomicU64,
}

impl Global for PreviewCache {}

impl PreviewCache {
    /// 启动时安装全局缓存：建实例目录、拿锁、顺手清理旧实例。
    ///
    /// ⚠️ 要在打开任何窗口**之前**调用（清理的是上一次运行留下的文件）。
    pub(crate) fn install(cx: &mut App) {
        cx.set_global(Self::new(&cache_root()));
    }

    pub(crate) fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    fn new(root: &Path) -> Self {
        let instance = root.join(instance_name());
        let _ = fs::create_dir_all(&instance);
        let lock = open_lock(&instance);
        let cache = Self {
            instance,
            _lock: lock,
            next: AtomicU64::new(0),
        };
        cache.clean_stale(root);
        cache
    }

    /// 一次「预览」用的缓存文件：最后一个租约释放时删除。
    pub(crate) fn allocate(&self, name: &str) -> CacheLease {
        CacheLease(Arc::new(LeaseInner {
            path: self.next_path(name),
        }))
    }

    /// 交给外部程序的副本：**不**挂租约，靠下次启动的实例清理回收。
    pub(crate) fn allocate_detached(&self, name: &str) -> PathBuf {
        self.next_path(name)
    }

    fn next_path(&self, name: &str) -> PathBuf {
        let seq = self.next.fetch_add(1, Ordering::Relaxed);
        self.instance.join(format!("{seq}-{}", safe_name(name)))
    }

    /// 清掉**确认无人使用**的旧实例目录。
    ///
    /// 拿得到锁 = 那个进程已经不在了；拿不到 = 还在跑，必须跳过。删不掉（文件被外部
    /// 程序占着、权限不足）也只是留在那儿，下次再说。
    fn clean_stale(&self, root: &Path) {
        let Ok(entries) = fs::read_dir(root) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path == self.instance || !path.is_dir() {
                continue;
            }
            if instance_alive(&path) {
                continue;
            }
            match fs::remove_dir_all(&path) {
                Ok(()) => log::debug!("已清理旧预览缓存:{}", path.display()),
                Err(error) => {
                    log::debug!("旧预览缓存暂时清不掉 {}:{error}", path.display())
                }
            }
        }
    }
}

/// 一份共享的缓存文件。
///
/// `Clone` 不会复制文件；最后一个句柄析构时才删除。文件不存在（下载失败 / 已被清理）
/// 也当成正常情况。
#[derive(Clone)]
pub(crate) struct CacheLease(Arc<LeaseInner>);

struct LeaseInner {
    path: PathBuf,
}

impl Drop for LeaseInner {
    fn drop(&mut self) {
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => log::debug!("清理预览缓存失败:{}:{error}", self.path.display()),
        }
    }
}

impl CacheLease {
    pub(crate) fn path(&self) -> &Path {
        &self.0.path
    }
}

/// `<系统缓存>/alacrterm/preview`。
///
/// 用缓存目录而不是 `temp_dir()`：有些平台会在开窗口的当口清理临时目录，而缓存目录
/// 至少是有归属的。
fn cache_root() -> PathBuf {
    let base = dirs::cache_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(std::env::temp_dir);
    base.join("alacrterm").join("preview")
}

/// 实例目录名：`<pid>-<启动时间>`。pid 保证同一时刻不重名，时间戳负责跨重启区分。
fn instance_name() -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    format!("{}-{stamp}", std::process::id())
}

/// 打开并**独占**实例目录的锁文件（`None` = 锁文件建不出来，只能放弃保护）。
fn open_lock(instance: &Path) -> Option<File> {
    let path = instance.join(LOCK_FILE);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| log::warn!("无法创建预览缓存锁 {}:{error}", path.display()))
        .ok()?;
    if let Err(error) = file.try_lock() {
        log::warn!("无法锁定预览缓存 {}:{error}", path.display());
        return None;
    }
    Some(file)
}

/// 这个实例目录还有活着的进程在用吗。
fn instance_alive(instance: &Path) -> bool {
    let path = instance.join(LOCK_FILE);
    let Ok(file) = OpenOptions::new().read(true).write(true).open(&path) else {
        // 没有锁文件：老版本留下的，或者建到一半就退出了 —— 都能清。
        return false;
    };
    // 能拿到锁 ⇒ 没人在用；`WouldBlock` ⇒ 那个实例还活着。
    file.try_lock().is_err()
}

/// 把远端文件名变成安全的本地文件名（远端路径里不该有分隔符，防一手）。
fn safe_name(name: &str) -> String {
    name.replace(['/', '\\'], "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "alacrterm-cache-test-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).expect("建测试目录");
        dir
    }

    #[test]
    fn lease_deletes_file_only_after_last_handle_drops() {
        let root = temp_root("lease");
        let cache = PreviewCache::new(&root);
        let lease = cache.allocate("a.txt");
        let path = lease.path().to_path_buf();
        fs::write(&path, b"x").expect("写缓存");

        let shared = lease.clone();
        drop(lease);
        assert!(path.exists(), "还有一份租约时不该删");

        drop(shared);
        assert!(!path.exists(), "最后一份租约释放后应当删除");
    }

    #[test]
    fn detached_file_survives_and_stale_instance_is_cleaned() {
        let root = temp_root("stale");
        let first = PreviewCache::new(&root);
        let detached = first.allocate_detached("media.mp4");
        fs::write(&detached, b"x").expect("写缓存");
        let first_instance = first.instance.clone();

        drop(first);
        assert!(detached.exists(), "外部程序那份不随实例析构删除");

        let second = PreviewCache::new(&root);
        assert!(!first_instance.exists(), "旧实例目录应当被清理");
        drop(second);
    }

    #[test]
    fn live_instance_is_not_cleaned() {
        let root = temp_root("live");
        let owner = PreviewCache::new(&root);
        let owner_instance = owner.instance.clone();

        let other = PreviewCache::new(&root);
        assert!(owner_instance.exists(), "锁被占用的实例目录不能被清理");
        drop(other);
        drop(owner);
    }
}
