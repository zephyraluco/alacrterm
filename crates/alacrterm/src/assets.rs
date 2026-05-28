use gpui::{AnyElement, App, AssetSource, IntoElement, RenderOnce, Result, SharedString, Window};
use gpui_component::{Icon, IconNamed};
use gpui_component::icon_named;
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