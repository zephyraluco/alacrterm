//! 自有图标资产：`assets/icons/*.svg` 经 [`icon_named!`] 生成 `IconName`，[`Assets`] 用 rust-embed 嵌入。

use gpui::{AnyElement, App, AssetSource, IntoElement, RenderOnce, Result, SharedString, Window};
use gpui_kit::component::{Icon, IconNamed, icon_named};
use rust_embed::RustEmbed;

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

/// gpui 资产源：嵌入 `assets/` 目录（`icons/**/*`）。
#[derive(RustEmbed)]
#[folder = "../../assets"]
#[include = "icons/**/*"]
pub struct Assets;

/// 默认按键绑定资产（见 [`crate::keymap`]）。
///
/// 单独一个 embed，且只收 `keymaps/default-*.json`——`assets/keymaps/` 下还有一堆
/// 未使用的 Zed 预设，没必要塞进二进制。
#[derive(RustEmbed)]
#[folder = "../../assets"]
#[include = "keymaps/default-*.json"]
pub struct KeymapAssets;

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
