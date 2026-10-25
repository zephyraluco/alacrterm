//! GPUI adapter for the process-local preview cache.
use gpui::{App, Global};
use std::{ops::Deref, path::PathBuf};
pub(crate) use util::file_cache::CacheLease;
pub(crate) struct PreviewCache(util::file_cache::FileCache);
impl Global for PreviewCache {}
impl PreviewCache {
    pub(crate) fn install(cx: &mut App) {
        cx.set_global(Self(util::file_cache::FileCache::new(&cache_root())));
    }
    pub(crate) fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }
}
impl Deref for PreviewCache {
    type Target = util::file_cache::FileCache;
    fn deref(&self) -> &Self::Target {
        &self.0
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
