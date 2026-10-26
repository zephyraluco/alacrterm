//! Prefixes outrank subsequence matches; case folding follows the active shell.
pub(super) fn match_score(query: &str, value: &str, insensitive: bool, fuzzy: bool) -> Option<i64> {
    if query == value {
        return None;
    }
    let folded_query = if insensitive {
        query.to_lowercase()
    } else {
        query.into()
    };
    let folded_value = if insensitive {
        value.to_lowercase()
    } else {
        value.into()
    };
    if folded_value.starts_with(&folded_query) {
        return Some(1000);
    }
    if !fuzzy || folded_query.is_empty() {
        return None;
    }
    let mut at = 0;
    let mut gaps = 0;
    for ch in folded_query.chars() {
        let offset = folded_value.get(at..)?.find(ch)?;
        gaps += offset;
        at += offset + ch.len_utf8();
    }
    Some(500 - (gaps as i64).min(400))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prefixes_outrank_fuzzy_matches_and_case_folding_is_explicit() {
        assert!(match_score("gi", "git", false, true) > match_score("gt", "git", false, true));
        assert!(match_score("get-i", "Get-Item", false, true).is_none());
        assert!(match_score("get-i", "Get-Item", true, true).is_some());
        assert!(match_score("zzz", "git", true, true).is_none());
    }
}
