use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor, Rgb as VteRgb};
use gpui::{Hsla, Rgba};

fn rgb(r: u8, g: u8, b: u8) -> Hsla {
    Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
    .into()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalTheme {
    pub foreground: Hsla,
    pub background: Hsla,
    pub cursor: Hsla,
    pub selection: Hsla,
    pub ansi: [Hsla; 16],
}

impl Default for TerminalTheme {
    fn default() -> Self {
        Self::tokyo_night_fallback()
    }
}

impl TerminalTheme {
    pub fn new(
        foreground: Hsla,
        background: Hsla,
        cursor: Hsla,
        selection: Hsla,
        ansi: [Hsla; 16],
    ) -> Self {
        Self {
            foreground,
            background,
            cursor,
            selection,
            ansi,
        }
    }

    fn tokyo_night_fallback() -> Self {
        Self {
            foreground: rgb(0xc0, 0xca, 0xf5),
            background: rgb(0x1a, 0x1b, 0x26),
            cursor: rgb(0x7a, 0xa2, 0xf7),
            selection: rgb(0x28, 0x3a, 0x5e),
            ansi: [
                rgb(0x1a, 0x1b, 0x26),
                rgb(0xf7, 0x76, 0x8e),
                rgb(0x9e, 0xce, 0x6a),
                rgb(0xe0, 0xaf, 0x68),
                rgb(0x7a, 0xa2, 0xf7),
                rgb(0xbb, 0x9a, 0xf7),
                rgb(0x7d, 0xcf, 0xff),
                rgb(0xa9, 0xb1, 0xd6),
                rgb(0x41, 0x48, 0x68),
                rgb(0xf7, 0x76, 0x8e),
                rgb(0x9e, 0xce, 0x6a),
                rgb(0xe0, 0xaf, 0x68),
                rgb(0x7a, 0xa2, 0xf7),
                rgb(0xbb, 0x9a, 0xf7),
                rgb(0x7d, 0xcf, 0xff),
                rgb(0xc0, 0xca, 0xf5),
            ],
        }
    }

    pub fn color_at_index(&self, index: usize) -> Hsla {
        match index {
            0..=15 => self.ansi[index],
            16..=231 => {
                let i = index as u8 - 16;
                let r = i / 36;
                let g = (i % 36) / 6;
                let b = i % 6;
                let to_val = |v: u8| -> u8 { if v == 0 { 0 } else { v * 40 + 55 } };
                rgb(to_val(r), to_val(g), to_val(b))
            }
            232..=255 => {
                let value = (index as u8 - 232) * 10 + 8;
                rgb(value, value, value)
            }
            256 => self.foreground,
            257 => self.background,
            258 => self.cursor,
            259..=266 => dim(self.ansi[index - 259]),
            267 => self.foreground,
            268 => dim(self.background),
            _ => self.foreground,
        }
    }
}

pub fn to_vte_rgb(color: Hsla) -> VteRgb {
    let color: Rgba = color.into();
    VteRgb {
        r: ((color.r * color.a) * 255.0) as u8,
        g: ((color.g * color.a) * 255.0) as u8,
        b: ((color.b * color.a) * 255.0) as u8,
    }
}

/// 解析 ANSI 颜色为 GPUI Hsla
/// is_bold: 当为 true 且使用标准调色板时，映射到亮色版本
pub fn resolve_color(color: &AnsiColor, is_bold: bool, theme: &TerminalTheme) -> Hsla {
    match color {
        AnsiColor::Named(named) => named_to_color(named, is_bold, theme),
        AnsiColor::Indexed(idx) => theme.color_at_index(*idx as usize),
        AnsiColor::Spec(rgb_val) => rgb(rgb_val.r, rgb_val.g, rgb_val.b),
    }
}

fn named_to_color(color: &NamedColor, is_bold: bool, theme: &TerminalTheme) -> Hsla {
    match color {
        NamedColor::Black => theme.ansi[if is_bold { 8 } else { 0 }],
        NamedColor::Red => theme.ansi[if is_bold { 9 } else { 1 }],
        NamedColor::Green => theme.ansi[if is_bold { 10 } else { 2 }],
        NamedColor::Yellow => theme.ansi[if is_bold { 11 } else { 3 }],
        NamedColor::Blue => theme.ansi[if is_bold { 12 } else { 4 }],
        NamedColor::Magenta => theme.ansi[if is_bold { 13 } else { 5 }],
        NamedColor::Cyan => theme.ansi[if is_bold { 14 } else { 6 }],
        NamedColor::White => theme.ansi[if is_bold { 15 } else { 7 }],
        NamedColor::BrightBlack => theme.ansi[8],
        NamedColor::BrightRed => theme.ansi[9],
        NamedColor::BrightGreen => theme.ansi[10],
        NamedColor::BrightYellow => theme.ansi[11],
        NamedColor::BrightBlue => theme.ansi[12],
        NamedColor::BrightMagenta => theme.ansi[13],
        NamedColor::BrightCyan => theme.ansi[14],
        NamedColor::BrightWhite => theme.ansi[15],
        NamedColor::Foreground => theme.foreground,
        NamedColor::Background => theme.background,
        NamedColor::Cursor => theme.cursor,
        NamedColor::BrightForeground => theme.foreground,
        NamedColor::DimBlack => dim(theme.ansi[0]),
        NamedColor::DimRed => dim(theme.ansi[1]),
        NamedColor::DimGreen => dim(theme.ansi[2]),
        NamedColor::DimYellow => dim(theme.ansi[3]),
        NamedColor::DimBlue => dim(theme.ansi[4]),
        NamedColor::DimMagenta => dim(theme.ansi[5]),
        NamedColor::DimCyan => dim(theme.ansi[6]),
        NamedColor::DimWhite => dim(theme.ansi[7]),
        _ => theme.foreground,
    }
}

fn dim(mut color: Hsla) -> Hsla {
    color.a *= 0.7;
    color
}
