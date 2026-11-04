//! 自有图标资产：`assets/icons/*.svg` 经 [`icon_named!`] 生成 `IconName`，[`Assets`] 用 rust-embed 嵌入。

use gpui::{
    AnyElement, App, AssetSource, Img, IntoElement, RenderOnce, Result, SharedString, Styled as _,
    Window, img, px,
};
use gpui_kit::component::{Icon, IconNamed, icon_named};
use rust_embed::RustEmbed;

icon_named!(IconName, "../../assets/icons");

impl From<IconName> for AnyElement {
    fn from(value: IconName) -> Self {
        ui_icon(value).into_any_element()
    }
}

impl RenderOnce for IconName {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        ui_icon(self)
    }
}

/// File-manager-sized icon geometry, including slots resized by the UI toolkit.
pub(crate) fn ui_icon(icon: impl Into<Icon>) -> Icon {
    Icon::new(icon)
        .w(px(16.))
        .h(px(16.))
        .min_w(px(16.))
        .min_h(px(16.))
        .max_w(px(16.))
        .max_h(px(16.))
        .flex_shrink_0()
}

/// Shared full-color SVG presentation for file, terminal and host icons.
pub(crate) fn colored_icon(path: &'static str) -> Img {
    img(SharedString::from(path))
        .w(px(16.))
        .h(px(16.))
        .flex_shrink_0()
}

/// gpui 资产源：嵌入操作图标与 Material Icon Theme 的文件图标。
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
        if path == "licenses/material-icons.md" {
            return Ok(Some(std::borrow::Cow::Borrowed(include_bytes!(
                "../../../docs/material-icons.md"
            ))));
        }
        Ok(Self::get(path).map(|f| f.data))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Self::iter()
            .chain(std::iter::once("licenses/material-icons.md".into()))
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
