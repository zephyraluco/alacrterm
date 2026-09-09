//! 自有图标资产：借鉴 alacrterm `crates/alacrterm/src/assets.rs` 的做法。
//!
//! 与直接使用 `gpui_kit::assets::Assets` 相比，这种方式把图标收归本仓库管理：
//! - `assets/icons/*.svg` —— 图标源文件（已从 gpui-kit-assets 拷贝，可自由增删自定义图标）
//! - [`icon_named!`] 扫描该目录生成本 crate 自己的 `IconName` 枚举（文件名 → PascalCase 变体，
//!   资产路径约定为 `icons/<文件名>`，与 gpui-kit 内置组件（如 TitleBar 窗口控制按钮）的
//!   内部图标路径一致）
//! - [`Assets`] 用 rust-embed 嵌入图标并实现 gpui 的 [`AssetSource`]，
//!   由 `application().with_assets(assets::Assets)` 注册
//!
//! 泛型实现 `impl<T: IconNamed> From<T> for Icon`（gpui-kit icon.rs）使自生成的
//! `IconName` 可直接传给 `Icon::new(...)` / `SidebarMenuItem::icon(...)` 等任何
//! 接受 `impl Into<Icon>` 的 API。

use gpui::{AnyElement, App, AssetSource, IntoElement, RenderOnce, Result, SharedString, Window};
use gpui_kit::component::{icon_named, Icon, IconNamed};
use rust_embed::RustEmbed;

// 调用宏扫描你自己的 Crate 目录下的自定义图标
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