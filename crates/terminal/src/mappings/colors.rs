use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor};
use gpui::{Hsla, Rgba};

// Tokyo Night 配色方案
// https://github.com/enkia/tokyo-night-vscode-theme

fn rgb(r: u8, g: u8, b: u8) -> Hsla {
    Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
    .into()
}

pub const DEFAULT_FG: fn() -> Hsla = || rgb(0xc0, 0xca, 0xf5);
pub const DEFAULT_BG: fn() -> Hsla = || rgb(0x1a, 0x1b, 0x26);
pub const CURSOR_COLOR: fn() -> Hsla = || rgb(0x7a, 0xa2, 0xf7);
pub const SELECTION_BG: fn() -> Hsla = || rgb(0x28, 0x3a, 0x5e);

/// 解析 ANSI 颜色为 GPUI Hsla
/// is_bold: 当为 true 且使用标准调色板时，映射到亮色版本
pub fn resolve_color(color: &AnsiColor, is_bold: bool) -> Hsla {
    match color {
        AnsiColor::Named(named) => named_to_color(named, is_bold),
        AnsiColor::Indexed(idx) => indexed_to_color(*idx),
        AnsiColor::Spec(rgb_val) => rgb(rgb_val.r, rgb_val.g, rgb_val.b),
    }
}

fn named_to_color(color: &NamedColor, is_bold: bool) -> Hsla {
    match color {
        // 标准 8 色（bold 时映射到亮色版本，符合传统终端行为）
        NamedColor::Black => {
            if is_bold {
                rgb(0x41, 0x48, 0x68)
            } else {
                rgb(0x1a, 0x1b, 0x26)
            }
        }
        NamedColor::Red => rgb(0xf7, 0x76, 0x8e),
        NamedColor::Green => rgb(0x9e, 0xce, 0x6a),
        NamedColor::Yellow => rgb(0xe0, 0xaf, 0x68),
        NamedColor::Blue => rgb(0x7a, 0xa2, 0xf7),
        NamedColor::Magenta => rgb(0xbb, 0x9a, 0xf7),
        NamedColor::Cyan => rgb(0x7d, 0xcf, 0xff),
        NamedColor::White => rgb(0xa9, 0xb1, 0xd6),

        // 亮色 8 色
        NamedColor::BrightBlack => rgb(0x41, 0x48, 0x68),
        NamedColor::BrightRed => rgb(0xf7, 0x76, 0x8e),
        NamedColor::BrightGreen => rgb(0x9e, 0xce, 0x6a),
        NamedColor::BrightYellow => rgb(0xe0, 0xaf, 0x68),
        NamedColor::BrightBlue => rgb(0x7a, 0xa2, 0xf7),
        NamedColor::BrightMagenta => rgb(0xbb, 0x9a, 0xf7),
        NamedColor::BrightCyan => rgb(0x7d, 0xcf, 0xff),
        NamedColor::BrightWhite => rgb(0xc0, 0xca, 0xf5),

        // 特殊颜色
        NamedColor::Foreground => rgb(0xc0, 0xca, 0xf5),
        NamedColor::Background => rgb(0x1a, 0x1b, 0x26),
        NamedColor::Cursor => rgb(0x7a, 0xa2, 0xf7),
        NamedColor::BrightForeground => rgb(0xcf, 0xd6, 0xe7),

        // Dim 颜色（暗淡版本）
        NamedColor::DimBlack => rgb(0x1a, 0x1b, 0x26),
        NamedColor::DimRed => rgb(0xb0, 0x55, 0x65),
        NamedColor::DimGreen => rgb(0x70, 0x90, 0x4a),
        NamedColor::DimYellow => rgb(0xa0, 0x7c, 0x4a),
        NamedColor::DimBlue => rgb(0x56, 0x72, 0xac),
        NamedColor::DimMagenta => rgb(0x85, 0x6d, 0xac),
        NamedColor::DimCyan => rgb(0x59, 0x91, 0xb3),
        NamedColor::DimWhite => rgb(0x77, 0x7d, 0x97),
        _ => rgb(0xc0, 0xca, 0xf5),
    }
}

fn indexed_to_color(index: u8) -> Hsla {
    match index {
        // 标准 16 色
        0 => rgb(0x1a, 0x1b, 0x26),
        1 => rgb(0xf7, 0x76, 0x8e),
        2 => rgb(0x9e, 0xce, 0x6a),
        3 => rgb(0xe0, 0xaf, 0x68),
        4 => rgb(0x7a, 0xa2, 0xf7),
        5 => rgb(0xbb, 0x9a, 0xf7),
        6 => rgb(0x7d, 0xcf, 0xff),
        7 => rgb(0xa9, 0xb1, 0xd6),
        8 => rgb(0x41, 0x48, 0x68),
        9 => rgb(0xf7, 0x76, 0x8e),
        10 => rgb(0x9e, 0xce, 0x6a),
        11 => rgb(0xe0, 0xaf, 0x68),
        12 => rgb(0x7a, 0xa2, 0xf7),
        13 => rgb(0xbb, 0x9a, 0xf7),
        14 => rgb(0x7d, 0xcf, 0xff),
        15 => rgb(0xc0, 0xca, 0xf5),

        // 6x6x6 颜色立方体（索引 16-231）
        16..=231 => {
            let i = index - 16;
            let r = i / 36;
            let g = (i % 36) / 6;
            let b = i % 6;
            let to_val = |v: u8| -> u8 { if v == 0 { 0 } else { v * 40 + 55 } };
            rgb(to_val(r), to_val(g), to_val(b))
        }

        // 灰度渐变（索引 232-255）
        232..=255 => {
            let value = (index - 232) * 10 + 8;
            rgb(value, value, value)
        }
    }
}
