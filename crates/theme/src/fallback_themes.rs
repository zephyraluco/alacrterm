use std::sync::Arc;

use gpui::{WindowBackgroundAppearance, hsla};

use crate::{
    Appearance, DEFAULT_DARK_THEME, SystemColors, Theme, ThemeColors, ThemeFamily,
    ThemeStyles, default_color_scales,
};

/// The default theme family for Zed.
///
/// This is used to construct the default theme fallback values, as well as to
/// have a theme available at compile time for tests.
pub fn zed_default_themes() -> ThemeFamily {
    ThemeFamily {
        id: "zed-default".to_string(),
        name: "Zed Default".into(),
        author: "".into(),
        themes: vec![zed_default_dark()],
        scales: default_color_scales(),
    }
}

pub(crate) fn zed_default_dark() -> Theme {
    let bg = hsla(215. / 360., 12. / 100., 15. / 100., 1.);
    let elevated_surface = hsla(225. / 360., 12. / 100., 17. / 100., 1.);
    let hover = hsla(225.0 / 360., 11.8 / 100., 26.7 / 100., 1.0);

    let blue = hsla(207.8 / 360., 81. / 100., 66. / 100., 1.0);
    let green = hsla(95. / 360., 38. / 100., 62. / 100., 1.0);
    let orange = hsla(29. / 360., 54. / 100., 61. / 100., 1.0);
    let purple = hsla(286. / 360., 51. / 100., 64. / 100., 1.0);
    let red = hsla(355. / 360., 65. / 100., 65. / 100., 1.0);
    let teal = hsla(187. / 360., 47. / 100., 55. / 100., 1.0);
    let yellow = hsla(39. / 360., 67. / 100., 69. / 100., 1.0);

    Theme {
        id: "one_dark".to_string(),
        name: DEFAULT_DARK_THEME.into(),
        appearance: Appearance::Dark,
        styles: ThemeStyles {
            window_background_appearance: WindowBackgroundAppearance::Opaque,
            system: SystemColors::default(),
            colors: ThemeColors {
                border: hsla(225. / 360., 13. / 100., 12. / 100., 1.),
                border_variant: hsla(228. / 360., 8. / 100., 25. / 100., 1.),
                border_focused: hsla(223. / 360., 78. / 100., 65. / 100., 1.),
                border_selected: hsla(222.6 / 360., 77.5 / 100., 65.1 / 100., 1.0),
                border_transparent: SystemColors::default().transparent,
                border_disabled: hsla(222.0 / 360., 11.6 / 100., 33.7 / 100., 1.0),
                elevated_surface_background: elevated_surface,
                surface_background: bg,
                background: bg,
                element_background: hsla(223.0 / 360., 13. / 100., 21. / 100., 1.0),
                element_hover: hover,
                element_active: hsla(220.0 / 360., 11.8 / 100., 20.0 / 100., 1.0),
                element_selected: hsla(224.0 / 360., 11.3 / 100., 26.1 / 100., 1.0),
                element_disabled: SystemColors::default().transparent,
                element_selection_background: blue.alpha(0.25),
                drop_target_background: hsla(220.0 / 360., 8.3 / 100., 21.4 / 100., 1.0),
                drop_target_border: hsla(221. / 360., 11. / 100., 86. / 100., 1.0),
                ghost_element_background: SystemColors::default().transparent,
                ghost_element_hover: hover,
                ghost_element_active: hsla(220.0 / 360., 11.8 / 100., 20.0 / 100., 1.0),
                ghost_element_selected: hsla(224.0 / 360., 11.3 / 100., 26.1 / 100., 1.0),
                ghost_element_disabled: SystemColors::default().transparent,
                text: hsla(221. / 360., 11. / 100., 86. / 100., 1.0),
                text_muted: hsla(218.0 / 360., 7. / 100., 46. / 100., 1.0),
                text_placeholder: hsla(220.0 / 360., 6.6 / 100., 44.5 / 100., 1.0),
                text_disabled: hsla(220.0 / 360., 6.6 / 100., 44.5 / 100., 1.0),
                text_accent: hsla(222.6 / 360., 77.5 / 100., 65.1 / 100., 1.0),
                icon: hsla(222.9 / 360., 9.9 / 100., 86.1 / 100., 1.0),
                icon_muted: hsla(220.0 / 360., 12.1 / 100., 66.1 / 100., 1.0),
                icon_disabled: hsla(220.0 / 360., 6.4 / 100., 45.7 / 100., 1.0),
                icon_placeholder: hsla(220.0 / 360., 6.4 / 100., 45.7 / 100., 1.0),
                icon_accent: blue,
                terminal_background: bg,
                // todo("Use one colors for terminal")
                terminal_ansi_background: crate::black().dark().step_12(),
                terminal_foreground: crate::white().dark().step_12(),
                terminal_bright_foreground: crate::white().dark().step_11(),
                terminal_dim_foreground: crate::white().dark().step_10(),
                terminal_ansi_black: crate::black().dark().step_12(),
                terminal_ansi_red: crate::red().dark().step_11(),
                terminal_ansi_green: crate::green().dark().step_11(),
                terminal_ansi_yellow: crate::yellow().dark().step_11(),
                terminal_ansi_blue: crate::blue().dark().step_11(),
                terminal_ansi_magenta: crate::violet().dark().step_11(),
                terminal_ansi_cyan: crate::cyan().dark().step_11(),
                terminal_ansi_white: crate::neutral().dark().step_12(),
                terminal_ansi_bright_black: crate::black().dark().step_11(),
                terminal_ansi_bright_red: crate::red().dark().step_10(),
                terminal_ansi_bright_green: crate::green().dark().step_10(),
                terminal_ansi_bright_yellow: crate::yellow().dark().step_10(),
                terminal_ansi_bright_blue: crate::blue().dark().step_10(),
                terminal_ansi_bright_magenta: crate::violet().dark().step_10(),
                terminal_ansi_bright_cyan: crate::cyan().dark().step_10(),
                terminal_ansi_bright_white: crate::neutral().dark().step_11(),
                terminal_ansi_dim_black: crate::black().dark().step_10(),
                terminal_ansi_dim_red: crate::red().dark().step_9(),
                terminal_ansi_dim_green: crate::green().dark().step_9(),
                terminal_ansi_dim_yellow: crate::yellow().dark().step_9(),
                terminal_ansi_dim_blue: crate::blue().dark().step_9(),
                terminal_ansi_dim_magenta: crate::violet().dark().step_9(),
                terminal_ansi_dim_cyan: crate::cyan().dark().step_9(),
                terminal_ansi_dim_white: crate::neutral().dark().step_10(),
            },
        },
    }
}
