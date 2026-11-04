//! Material Icon Theme associations for remote filesystem entries.

use std::{collections::BTreeMap, sync::LazyLock};

use gpui::{App, Img};
use gpui_kit::component::ActiveTheme as _;
use serde::Deserialize;

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Associations {
    file: Option<String>,
    folder: Option<String>,
    folder_expanded: Option<String>,
    #[serde(default)]
    file_names: BTreeMap<String, String>,
    #[serde(default)]
    file_extensions: BTreeMap<String, String>,
    #[serde(default)]
    folder_names: BTreeMap<String, String>,
    #[serde(default)]
    folder_names_expanded: BTreeMap<String, String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    #[serde(flatten)]
    default: Associations,
    light: Associations,
    icon_definitions: BTreeMap<String, String>,
}

static MANIFEST: LazyLock<Manifest> = LazyLock::new(|| {
    let mut manifest: Manifest =
        serde_json::from_str(include_str!("../../../assets/icons/material/manifest.json"))
            .unwrap_or_default();
    // Upstream contains mixed-case keys; apply the same normalization as lookup
    // once, including light-theme overrides and compound extension mappings.
    for associations in [&mut manifest.default, &mut manifest.light] {
        for table in [
            &mut associations.file_names,
            &mut associations.file_extensions,
            &mut associations.folder_names,
            &mut associations.folder_names_expanded,
        ] {
            *table = std::mem::take(table)
                .into_iter()
                .map(|(key, icon)| (key.to_lowercase(), icon))
                .collect();
        }
    }
    manifest
});

fn associated_icon<'a>(
    defaults: &'a Associations,
    light: Option<&'a Associations>,
    name: &str,
    directory: bool,
    expanded: bool,
) -> Option<&'a str> {
    let lookup = |normal: &'a BTreeMap<String, String>,
                  variants: Option<&'a BTreeMap<String, String>>,
                  key: &str| {
        normal.get(key).map(|icon| {
            variants
                .and_then(|map| map.get(key))
                .unwrap_or(icon)
                .as_str()
        })
    };
    if directory {
        return if expanded {
            lookup(
                &defaults.folder_names_expanded,
                light.map(|theme| &theme.folder_names_expanded),
                name,
            )
        } else {
            lookup(
                &defaults.folder_names,
                light.map(|theme| &theme.folder_names),
                name,
            )
        };
    }
    if let Some(icon) = lookup(
        &defaults.file_names,
        light.map(|theme| &theme.file_names),
        name,
    ) {
        return Some(icon);
    }
    // Resolve long compound suffixes before shorter ones, without allowing a
    // generic light-theme association to override an exact default filename.
    name.match_indices('.').find_map(|(position, _)| {
        lookup(
            &defaults.file_extensions,
            light.map(|theme| &theme.file_extensions),
            &name[position + 1..],
        )
    })
}

fn icon_path(name: &str, directory: bool, expanded: bool, light: bool) -> &'static str {
    let manifest = &*MANIFEST;
    let name = name.rsplit('/').next().unwrap_or(name).to_lowercase();
    let icon = associated_icon(
        &manifest.default,
        light.then_some(&manifest.light),
        &name,
        directory,
        expanded,
    )
    .or(match (directory, expanded) {
        (true, true) => manifest.default.folder_expanded.as_deref(),
        (true, false) => manifest.default.folder.as_deref(),
        (false, _) => manifest.default.file.as_deref(),
    });
    icon.and_then(|id| manifest.icon_definitions.get(id))
        .map(String::as_str)
        .unwrap_or("icons/material/svg/file.svg")
}

pub(crate) fn file_icon(name: &str, directory: bool, expanded: bool, cx: &App) -> Img {
    // img preserves SVG colors; the UI Icon component renders a monochrome mask.
    crate::assets::colored_icon(icon_path(
        name,
        directory,
        expanded,
        !cx.theme().mode.is_dark(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::Assets;

    #[test]
    fn exact_names_and_compound_suffixes_win_over_generic_extensions() {
        assert!(icon_path("package.json", false, false, false).ends_with("/nodejs.svg"));
        assert!(icon_path("package.json", false, false, true).ends_with("/nodejs.svg"));
        assert!(icon_path("index.d.ts", false, false, false).ends_with("/typescript-def.svg"));
        assert!(icon_path("APP.PY", false, false, false).ends_with("/python.svg"));
        assert!(icon_path(".gitignore", false, false, false).ends_with("/git.svg"));
    }

    #[test]
    fn mixed_case_manifest_names_keep_their_specific_icons() {
        for (name, icon) in [
            ("CLAUDE.md", "claude"),
            ("Caddyfile", "caddy"),
            ("PKGBUILD", "console"),
            ("CMakePresets.json", "cmake"),
        ] {
            for light in [false, true] {
                assert!(
                    icon_path(name, false, false, light).ends_with(&format!("/{icon}.svg")),
                    "{name}"
                );
                assert_eq!(
                    icon_path(name, false, false, light),
                    icon_path(&name.to_lowercase(), false, false, light)
                );
            }
        }
    }

    #[test]
    fn light_theme_uses_the_official_variant() {
        assert!(icon_path("Cargo.toml", false, false, false).ends_with("/toml.svg"));
        assert!(icon_path("Cargo.toml", false, false, true).ends_with("/toml_light.svg"));
    }

    #[test]
    fn folders_and_unknown_files_have_embedded_fallbacks() {
        assert!(icon_path("src", true, false, false).ends_with("/folder-src.svg"));
        assert!(icon_path("src", true, true, false).ends_with("/folder-src-open.svg"));
        assert!(icon_path("unrecognized", true, false, false).ends_with("/folder.svg"));
        assert!(
            icon_path("unrecognized.weird-extension", false, false, false).ends_with("/file.svg")
        );
        assert!(icon_path("no-extension", false, false, false).ends_with("/file.svg"));
    }

    #[test]
    fn every_imported_association_resolves_to_an_embedded_svg() {
        let manifest: Manifest =
            serde_json::from_str(include_str!("../../../assets/icons/material/manifest.json"))
                .unwrap();
        for associations in [&manifest.default, &manifest.light] {
            for id in associations
                .file
                .iter()
                .chain(&associations.folder)
                .chain(&associations.folder_expanded)
                .chain(associations.file_names.values())
                .chain(associations.file_extensions.values())
                .chain(associations.folder_names.values())
                .chain(associations.folder_names_expanded.values())
            {
                let path = &manifest.icon_definitions[id];
                let asset = Assets::get(path).unwrap_or_else(|| panic!("missing icon: {path}"));
                assert!(
                    std::str::from_utf8(&asset.data).unwrap().contains("<svg"),
                    "{path}"
                );
            }
        }
    }
}
