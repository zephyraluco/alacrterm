//! Terminal palettes, embedded for packaged builds with optional file overrides.
use serde::Deserialize;
use std::collections::HashMap;

pub const SCHEMES: &[(&str, &str)] = &[
    ("catppuccin-mocha", "Catppuccin Mocha"),
    ("tokyo-night", "Tokyo Night"),
    ("one-dark", "One Dark"),
    ("nord", "Nord"),
    ("gruvbox-dark", "Gruvbox Dark"),
    ("solarized-dark", "Solarized Dark"),
];

#[derive(Debug, Deserialize)]
pub struct ColorScheme {
    pub name: String,
    pub colors: HashMap<String, String>,
    pub selection_color: String,
    pub search_match_color: String,
    pub link_color: String,
}

/// Parse RGB/RGBA hexadecimal colors without slicing untrusted UTF-8.
pub fn parse_hex(value: &str) -> Option<[u8; 4]> {
    let hex = value.trim().trim_start_matches('#');
    if !hex.is_ascii() || !matches!(hex.len(), 6 | 8) {
        return None;
    }
    Some([
        u8::from_str_radix(&hex[0..2], 16).ok()?,
        u8::from_str_radix(&hex[2..4], 16).ok()?,
        u8::from_str_radix(&hex[4..6], 16).ok()?,
        if hex.len() == 8 {
            u8::from_str_radix(&hex[6..8], 16).ok()?
        } else {
            255
        },
    ])
}

fn parse(text: &str) -> Option<ColorScheme> {
    let scheme: ColorScheme = serde_json::from_str(text).ok()?;
    for key in [
        "terminal_background",
        "terminal_foreground",
        "terminal_bright_foreground",
    ] {
        parse_hex(scheme.colors.get(key)?)?;
    }
    for prefix in ["", "bright_", "dim_"] {
        for name in [
            "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
        ] {
            parse_hex(
                scheme
                    .colors
                    .get(&format!("terminal_ansi_{prefix}{name}"))?,
            )?;
        }
    }
    for color in [
        &scheme.selection_color,
        &scheme.search_match_color,
        &scheme.link_color,
    ] {
        parse_hex(color)?;
    }
    Some(scheme)
}

fn embedded(id: &str) -> Option<&'static str> {
    Some(match id {
        "catppuccin-mocha" => include_str!("../../../terminal-colors/catppuccin-mocha.json"),
        "tokyo-night" => include_str!("../../../terminal-colors/tokyo-night.json"),
        "one-dark" => include_str!("../../../terminal-colors/one-dark.json"),
        "nord" => include_str!("../../../terminal-colors/nord.json"),
        "gruvbox-dark" => include_str!("../../../terminal-colors/gruvbox-dark.json"),
        "solarized-dark" => include_str!("../../../terminal-colors/solarized-dark.json"),
        _ => return None,
    })
}

/// Unknown IDs are ignored; missing or invalid overrides use the embedded palette.
pub fn load(id: &str) -> Option<ColorScheme> {
    let fallback = embedded(id)?;
    if let Some((_, text)) = crate::read_config_file(&format!("terminal-colors/{id}.json"))
        && let Some(scheme) = parse(&text)
    {
        return Some(scheme);
    }
    parse(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_six_palettes_have_valid_complete_colors() {
        for (id, name) in SCHEMES {
            let scheme = parse(embedded(id).unwrap()).unwrap();
            assert_eq!(&scheme.name, name);
            assert_eq!(scheme.colors.len(), 27);
        }
        assert!(load("../../config/app").is_none());
    }
    #[test]
    fn invalid_or_incomplete_palettes_are_rejected() {
        assert!(parse(r#"{"name":"broken","colors":{}}"#).is_none());
        for value in ["#GG0000", "#界界", "12345", "é00000"] {
            assert!(parse_hex(value).is_none());
        }
        assert_eq!(parse_hex("#11223380"), Some([17, 34, 51, 128]));
    }
}
