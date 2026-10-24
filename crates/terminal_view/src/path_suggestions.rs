//! Parse only literal path tokens; shell expressions remain native completions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PathQuery {
    pub input: String,
    pub directory: String,
    pub leaf: String,
    quote: Option<char>,
    powershell: bool,
}
#[derive(Clone, Debug)]
pub(super) struct Entry {
    pub name: String,
    pub directory: bool,
}
impl PathQuery {
    pub fn parse(input: &str, powershell: bool) -> Option<Self> {
        let escape = if powershell { '`' } else { '\\' };
        let mut quote = None;
        let mut escaped = false;
        let mut value = String::new();
        let mut arguments = 0;
        for ch in input.chars() {
            if ch.is_control() {
                return None;
            }
            if escaped {
                if powershell && ch.is_ascii_alphabetic() {
                    return None;
                }
                if !powershell && quote == Some('"') && !"\"$`\\".contains(ch) {
                    return None;
                }
                value.push(ch);
                escaped = false;
                continue;
            }
            if ch == escape && quote != Some('\'') {
                escaped = true;
                continue;
            }
            if let Some(current) = quote {
                // Closed quoted tokens need replacement rather than suffix insertion.
                if ch == current {
                    return None;
                } else if current == '"' && "$`".contains(ch) {
                    return None;
                } else {
                    value.push(ch);
                }
            } else if ch == '\'' || ch == '"' {
                quote = Some(ch);
            } else if ch.is_whitespace() {
                if !value.is_empty() {
                    arguments += 1;
                    value.clear();
                }
            } else if ";|&<>$`(){}[]*?!".contains(ch) {
                return None;
            } else {
                value.push(ch);
            }
        }
        if escaped || value.starts_with('-') {
            return None;
        }
        let split = value.rfind(|ch| ch == '/' || (powershell && ch == '\\'));
        if arguments == 0 && split.is_none() {
            return None;
        }
        let (directory, leaf) = match split {
            Some(at) => (&value[..=at], &value[at + 1..]),
            None => ("", value.as_str()),
        };
        Some(Self {
            input: input.into(),
            directory: directory.into(),
            leaf: leaf.into(),
            quote,
            powershell,
        })
    }
    pub fn candidates(&self, mut entries: Vec<Entry>) -> Vec<String> {
        entries.sort_by(|a, b| {
            b.directory
                .cmp(&a.directory)
                .then_with(|| a.name.cmp(&b.name))
        });
        entries
            .into_iter()
            .filter(|entry| {
                entry.name.starts_with(&self.leaf) && !entry.name.chars().any(char::is_control)
            })
            .filter(|entry| !entry.name.starts_with('.') || self.leaf.starts_with('.'))
            .take(64)
            .map(|entry| {
                let tail = &entry.name[self.leaf.len()..];
                let mut output = self.input.clone();
                for ch in tail.chars() {
                    match self.quote {
                        Some('\'') if ch == '\'' => {
                            output.push_str(if self.powershell { "''" } else { "'\\''" })
                        }
                        Some('"') if "\"$`".contains(ch) || (!self.powershell && ch == '\\') => {
                            output.push(if self.powershell { '`' } else { '\\' });
                            output.push(ch);
                        }
                        None if !(ch.is_alphanumeric() || "_./:-".contains(ch)) => {
                            output.push(if self.powershell { '`' } else { '\\' });
                            output.push(ch);
                        }
                        _ => output.push(ch),
                    }
                }
                if entry.directory {
                    output.push('/');
                } else if let Some(quote) = self.quote {
                    output.push(quote);
                }
                output
            })
            .filter(|candidate| *candidate != self.input)
            .collect()
    }
}

pub(super) fn remote_directory(cwd: &str, directory: &str, home: Option<&str>) -> Option<String> {
    if directory.starts_with('/') {
        return Some(directory.into());
    }
    if directory.starts_with("~/") {
        return Some(format!("{}/{}", home?, &directory[2..]));
    }
    if !cwd.starts_with('/') {
        return None;
    }
    Some(format!("{}/{}", cwd.trim_end_matches('/'), directory))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn file(name: &str) -> Entry {
        Entry {
            name: name.into(),
            directory: false,
        }
    }
    #[test]
    fn literal_tokens_escape_suffixes_without_evaluating_shell_expressions() {
        let query = PathQuery::parse("cat My", false).unwrap();
        assert_eq!(
            query.candidates(vec![file("My File.txt")]),
            ["cat My\\ File.txt"]
        );
        let query = PathQuery::parse("cat \"My ", false).unwrap();
        assert_eq!(
            query.candidates(vec![file("My File.txt")]),
            ["cat \"My File.txt\""]
        );
        let query = PathQuery::parse("cat 'it", false).unwrap();
        assert_eq!(
            query.candidates(vec![file("it's.txt")]),
            ["cat 'it'\\''s.txt'"]
        );
        assert!(PathQuery::parse("cat $(secret)", false).is_none());
        assert!(PathQuery::parse("cat *.txt", false).is_none());
        assert!(PathQuery::parse("git", false).is_none());
        assert!(PathQuery::parse("cat \"My\"", false).is_none());
        assert!(PathQuery::parse("cat 'it''s", true).is_none());
        assert_eq!(
            remote_directory("/remote", "../", None).as_deref(),
            Some("/remote/../")
        );
        assert!(remote_directory("unknown", "", None).is_none());
    }
    #[test]
    fn powershell_and_directory_completions_preserve_open_quotes() {
        let query = PathQuery::parse("Get-Content My", true).unwrap();
        assert_eq!(
            query.candidates(vec![file("My File")]),
            ["Get-Content My` File"]
        );
        let query = PathQuery::parse("cd \"My", false).unwrap();
        assert_eq!(
            query.candidates(vec![Entry {
                name: "My Folder".into(),
                directory: true
            }]),
            ["cd \"My Folder/"]
        );
    }
}
