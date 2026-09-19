//! 自有图标资产：`assets/icons/*.svg` 由 [`icon_named!`] 扫成 `IconName` 枚举（资产路径
//! 约定为 `icons/<文件名>`，与 gpui-kit 内置组件的图标路径一致），[`Assets`] 用 rust-embed
//! 嵌入并实现 gpui 的 [`AssetSource`]，由 `application().with_assets(assets::Assets)` 注册。

use gpui::{AnyElement, App, AssetSource, IntoElement, RenderOnce, Result, SharedString, Window};
use gpui_kit::component::{icon_named, Icon, IconNamed};
use rust_embed::RustEmbed;

// 扫描 `assets/icons` 生成本 crate 的 `IconName` 枚举
icon_named!(IconName, "../../assets/icons");

impl From<IconName> for AnyElement {
    fn from(value: IconName) -> Self {
        Icon::new(value).into_any_element()
    }
}

impl RenderOnce for IconName {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        Icon::new(self)
    }
}

/// 嵌入 `assets/` 目录（`icons/**/*`），作为 gpui 的资产源。
#[derive(RustEmbed)]
#[folder = "../../assets"]
#[include = "icons/**/*"]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        Ok(Self::get(path).map(|f| f.data))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Self::iter()
            .filter_map(|p| {
                if p.starts_with(path) {
                    Some(p.into())
                } else {
                    None
                }
            })
            .collect())
    }
}