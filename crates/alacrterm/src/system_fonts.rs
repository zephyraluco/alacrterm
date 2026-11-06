//! Native system font discovery. Return names only; native font objects stay on
//! the background thread that created them.
use font_kit::source::SystemSource;

pub(crate) fn scan() -> Result<Vec<String>, String> {
    let handles = SystemSource::new()
        .all_fonts()
        .map_err(|error| format!("无法读取系统字体：{error}"))?;
    let mut faces = Vec::with_capacity(handles.len());
    let mut skipped = 0;
    for handle in handles {
        match handle.load() {
            Ok(font) => faces.push((font.family_name(), font.is_monospace())),
            Err(_) => skipped += 1,
        }
    }
    if skipped > 0 {
        log::debug!("Skipped {skipped} unreadable system font faces");
    }
    Ok(monospace_families(faces))
}

fn monospace_families(faces: impl IntoIterator<Item = (String, bool)>) -> Vec<String> {
    let mut families: Vec<_> = faces
        .into_iter()
        .filter_map(|(name, monospace)| (monospace && !name.trim().is_empty()).then_some(name))
        .collect();
    families.sort_by_cached_key(|name| (name.to_lowercase(), name.clone()));
    families.dedup_by(|a, b| a.to_lowercase() == b.to_lowercase());
    families
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovered_families_resolve_to_native_monospace_faces() {
        let families = scan().expect("system font enumeration failed");
        let source = SystemSource::new();
        for name in &families {
            let family = source
                .select_family_by_name(name)
                .expect("scanned family must be selectable by its name");
            assert!(
                family
                    .fonts()
                    .iter()
                    .any(|handle| { handle.load().is_ok_and(|font| font.is_monospace()) }),
                "{name} must have a native monospace face"
            );
        }
        eprintln!("Scanned {} system monospace families", families.len());
    }

    #[test]
    fn font_metadata_not_family_name_determines_monospace() {
        let names = monospace_families([
            ("Looks Like Mono".into(), false),
            ("Consolas".into(), true),
            ("Arial".into(), false),
            ("".into(), true),
            ("  ".into(), true),
        ]);
        assert_eq!(names, ["Consolas"]);
    }

    #[test]
    fn variants_are_deduplicated_and_families_have_stable_order() {
        let names = monospace_families([
            ("Menlo".into(), true),
            ("Consolas".into(), true),
            ("Menlo".into(), true),
            ("consolas".into(), true),
            ("DejaVu Sans Mono".into(), true),
        ]);
        assert_eq!(names, ["Consolas", "DejaVu Sans Mono", "Menlo"]);
    }
}
