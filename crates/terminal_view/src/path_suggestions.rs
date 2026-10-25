//! Literal shell token completion. Expressions are left to the native shell.
use crate::completion::{Candidate, Kind, Source};
use std::ops::Range;
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PathQuery {
    pub input: String,
    pub cursor: usize,
    pub directory: String,
    pub leaf: String,
    pub expand_home: bool,
    pub directories_only: bool,
    pub replacement: Range<usize>,
    quote: Option<char>,
    powershell: bool,
}
#[derive(Clone, Debug)]
pub(super) struct Entry {
    pub name: String,
    pub directory: bool,
}
#[derive(Clone, Debug)]
struct Token {
    range: Range<usize>,
    value: String,
}
/// Parse literal words including adjacent quoted segments. No evaluation occurs.
fn tokens(input: &str, powershell: bool) -> Option<Vec<Token>> {
    let mut result = Vec::new();
    let mut value = String::new();
    let mut start = None;
    let mut quote = None;
    let mut chars = input.char_indices().peekable();
    while let Some((at, ch)) = chars.next() {
        if ch.is_control() {
            return None;
        }
        if ch.is_whitespace() && quote.is_none() {
            if let Some(start) = start.take() {
                result.push(Token {
                    range: start..at,
                    value: std::mem::take(&mut value),
                });
            }
            continue;
        }
        start.get_or_insert(at);
        let escape = if powershell { '`' } else { '\\' };
        if ch == escape && quote != Some('\'') {
            let (_, next) = chars.next()?;
            if next.is_control() || powershell && next.is_ascii_alphabetic() {
                return None;
            }
            if !powershell && quote == Some('"') && !"\"$`\\".contains(next) {
                value.push(ch);
            }
            value.push(next);
            continue;
        }
        if let Some(current) = quote {
            if ch == current {
                if powershell
                    && current == '\''
                    && chars.peek().is_some_and(|(_, next)| *next == '\'')
                {
                    chars.next();
                    value.push('\'');
                } else {
                    quote = None;
                }
            } else if current == '"' && "$`".contains(ch) {
                return None;
            } else {
                value.push(ch);
            }
        } else if matches!(ch, '\'' | '"') {
            quote = Some(ch);
        } else if ";|&<>$`(){}[]*?!#".contains(ch) {
            return None;
        } else {
            value.push(ch);
        }
    }
    if let Some(start) = start {
        result.push(Token {
            range: start..input.len(),
            value,
        });
    }
    Some(result)
}
impl PathQuery {
    pub fn parse(input: &str, cursor: usize, powershell: bool) -> Option<Self> {
        let before = input.get(..cursor)?;
        let prefix_words = tokens(before, powershell)?;
        let all_words = tokens(input, powershell)?;
        let current = prefix_words
            .last()
            .filter(|token| token.range.end == cursor);
        let start = current.map_or(cursor, |token| token.range.start);
        let value = current.map_or("", |token| token.value.as_str());
        let end = all_words
            .iter()
            .find(|token| token.range.start == start)
            .map_or(cursor, |token| token.range.end);
        let argument = all_words
            .iter()
            .filter(|token| token.range.start < start)
            .count();
        if value.starts_with('-') {
            return None;
        }
        let split = value.rfind(|ch| ch == '/' || (powershell && ch == '\\'));
        if argument == 0 && split.is_none() {
            return None;
        }
        let (directory, leaf) = match split {
            Some(at) => (&value[..=at], &value[at + 1..]),
            None => ("", value),
        };
        let raw = input.get(start..end)?;
        let quote = raw.chars().next().filter(|ch| matches!(ch, '\'' | '"'));
        let command = all_words.first()?.value.to_lowercase();
        Some(Self {
            input: input.into(),
            cursor,
            directory: directory.into(),
            leaf: leaf.into(),
            expand_home: raw.starts_with("~/"),
            directories_only: matches!(
                command.as_str(),
                "cd" | "pushd" | "set-location" | "sl" | "push-location"
            ),
            replacement: start..end,
            quote,
            powershell,
        })
    }
    pub fn candidates(&self, entries: Vec<Entry>) -> Vec<Candidate> {
        entries
            .into_iter()
            .filter(|entry| {
                let matches = if self.powershell {
                    entry
                        .name
                        .to_lowercase()
                        .starts_with(&self.leaf.to_lowercase())
                } else {
                    entry.name.starts_with(&self.leaf)
                };
                matches
                    && !entry.name.chars().any(char::is_control)
                    && (!self.directories_only || entry.directory)
                    && (!entry.name.starts_with('.') || self.leaf.starts_with('.'))
            })
            .map(|entry| {
                let logical = format!(
                    "{}{}{}",
                    self.directory,
                    entry.name,
                    if entry.directory { "/" } else { "" }
                );
                let mut encoded = String::new();
                if let Some(quote) = self.quote {
                    encoded.push(quote);
                }
                for (index, ch) in logical.chars().enumerate() {
                    match self.quote {
                        Some('\'') if ch == '\'' => {
                            encoded.push_str(if self.powershell { "''" } else { "'\\''" })
                        }
                        Some('"') if "\"$`".contains(ch) || (!self.powershell && ch == '\\') => {
                            encoded.push(if self.powershell { '`' } else { '\\' });
                            encoded.push(ch);
                        }
                        None if !(ch.is_alphanumeric()
                            || "_./:-".contains(ch)
                            || ch == '~' && index == 0 && self.expand_home) =>
                        {
                            encoded.push(if self.powershell { '`' } else { '\\' });
                            encoded.push(ch);
                        }
                        _ => encoded.push(ch),
                    }
                }
                if let Some(quote) = self.quote {
                    encoded.push(quote);
                }
                let kind = if entry.directory {
                    Kind::Directory
                } else {
                    Kind::File
                };
                let mut item = Candidate::new(
                    encoded,
                    0,
                    kind,
                    Source::Filesystem,
                    if entry.directory { 1950 } else { 1900 },
                );
                item.label = format!("{}{}", entry.name, if entry.directory { "/" } else { "" });
                item.replacement = self.replacement.clone();
                item.cursor_back = usize::from(entry.directory && self.quote.is_some());
                item
            })
            .filter(|item| {
                self.input.get(item.replacement.clone()) != Some(item.insert_text.as_str())
            })
            .collect()
    }
    pub fn remote_directory(&self, cwd: &str, home: Option<&str>) -> Option<String> {
        if self.directory.starts_with('/') {
            return Some(self.directory.clone());
        }
        if self.expand_home {
            return Some(format!("{}/{}", home?, self.directory.strip_prefix("~/")?));
        }
        if !cwd.starts_with('/') {
            return None;
        }
        Some(format!("{}/{}", cwd.trim_end_matches('/'), self.directory))
    }
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
    fn query(input: &str) -> PathQuery {
        PathQuery::parse(input, input.len(), false).unwrap()
    }
    #[test]
    fn escapes_literal_names_and_handles_closed_quotes_in_previous_arguments() {
        assert_eq!(
            query("cat My").candidates(vec![file("My File.txt")])[0].insert_text,
            "My\\ File.txt"
        );
        assert_eq!(
            query("cat 'it").candidates(vec![file("it's.txt")])[0].insert_text,
            "'it'\\''s.txt'"
        );
        assert_eq!(query("cat \"a.txt\" ./b").directory, "./");
        assert_eq!(
            query("cat \"My\"").candidates(vec![file("My File")])[0].insert_text,
            "\"My File\""
        );
        assert!(PathQuery::parse("cat $(secret)", 13, false).is_none());
        assert!(PathQuery::parse("cat *.txt", 9, false).is_none());
    }
    #[test]
    fn home_expansion_respects_quotes_and_directory_commands_filter_files() {
        assert!(!query("cat \"~/Do").expand_home);
        assert_eq!(
            query("cat \"~/Do")
                .remote_directory("/remote", Some("/home/me"))
                .as_deref(),
            Some("/remote/~/")
        );
        assert_eq!(
            query("cat ~/Do")
                .remote_directory("/remote", Some("/home/me"))
                .as_deref(),
            Some("/home/me/")
        );
        let query = query("cd sam");
        let result = query.candidates(vec![
            file("sample.txt"),
            Entry {
                name: "samples".into(),
                directory: true,
            },
        ]);
        assert_eq!(result.len(), 1);
        assert!(result[0].retrigger);
    }
    #[test]
    fn middle_word_replaces_the_entire_token_without_touching_following_arguments() {
        let query = PathQuery::parse("cat saZZ other", 6, false).unwrap();
        assert_eq!(query.replacement, 4..8);
        assert_eq!(query.leaf, "sa");
        assert_eq!(
            query.candidates(vec![file("sample")])[0].insert_text,
            "sample"
        );
        let query = PathQuery::parse("Get-Content 'it''s", 99, true);
        assert!(query.is_none()); // invalid cursor boundary
        let input = "Get-Content 'it''s";
        let query = PathQuery::parse(input, input.len(), true).unwrap();
        assert_eq!(
            query.candidates(vec![file("it's a file")])[0].insert_text,
            "'it''s a file'"
        );
    }
}
