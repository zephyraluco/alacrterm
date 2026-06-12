pub mod paths;
pub mod rel_path;
pub mod shell;

use std::{
    borrow::Cow,
    path::{Component, Path, PathBuf},
};

pub use self::shell::{
    get_default_system_shell, get_default_system_shell_preferring_bash, get_system_shell,
};

pub trait ResultExt<T> {
    fn log_err(self) -> Option<T>;
}

impl<T, E> ResultExt<T> for Result<T, E>
where
    E: std::fmt::Display,
{
    fn log_err(self) -> Option<T> {
        match self {
            Ok(value) => Some(value),
            Err(error) => {
                log::error!("{error}");
                None
            }
        }
    }
}

pub trait TryFutureExt: Sized {
    fn detach_and_log_err<C>(self, cx: &mut C)
    where
        C: ?Sized;
}

#[macro_export]
macro_rules! debug_panic {
    ($($arg:tt)*) => {{
        debug_assert!(false, $($arg)*);
        log::error!($($arg)*);
    }};
}

pub fn truncate_and_trailoff(s: &str, max_chars: usize) -> String {
    debug_assert!(max_chars >= 5);

    if s.len() <= max_chars {
        return s.to_string();
    }

    match s.char_indices().map(|(i, _)| i).nth(max_chars) {
        Some(index) => s[..index].to_string() + "…",
        None => s.to_string(),
    }
}

pub fn default<D: Default>() -> D {
    Default::default()
}

pub fn normalize_path(path: &Path) -> PathBuf {
    let mut components = path.components().peekable();
    let mut ret = if let Some(c @ Component::Prefix(..)) = components.peek().cloned() {
        components.next();
        PathBuf::from(c.as_os_str())
    } else {
        PathBuf::new()
    };

    for component in components {
        match component {
            Component::Prefix(..) => unreachable!(),
            Component::RootDir => ret.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                ret.pop();
            }
            Component::Normal(c) => ret.push(c),
        }
    }

    ret
}

// Get an embedded file as a string.
pub fn asset_str<A: rust_embed::RustEmbed>(path: &str) -> Cow<'static, str> {
    match A::get(path).expect(path).data {
        Cow::Borrowed(bytes) => Cow::Borrowed(std::str::from_utf8(bytes).unwrap()),
        Cow::Owned(bytes) => Cow::Owned(String::from_utf8(bytes).unwrap()),
    }
}

pub fn strip_json_comments(content: &str) -> String {
    let mut without_comments = String::with_capacity(content.len());
    let mut chars = content.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    let mut pending_comma: Option<String> = None;

    while let Some(ch) = chars.next() {
        if in_string {
            without_comments.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        if let Some(comma) = pending_comma.as_mut() {
            if ch.is_whitespace() {
                comma.push(ch);
                continue;
            }

            if ch == '/' {
                match chars.peek() {
                    Some('/') => {
                        chars.next();
                        while let Some(comment_ch) = chars.next() {
                            if comment_ch == '\n' {
                                comma.push('\n');
                                break;
                            }
                        }
                        continue;
                    }
                    Some('*') => {
                        chars.next();
                        let mut previous = '\0';
                        while let Some(comment_ch) = chars.next() {
                            if comment_ch == '\n' {
                                comma.push('\n');
                            }
                            if previous == '*' && comment_ch == '/' {
                                break;
                            }
                            previous = comment_ch;
                        }
                        continue;
                    }
                    _ => {}
                }
            }

            if matches!(ch, '}' | ']') {
                pending_comma = None;
                without_comments.push(ch);
                continue;
            }

            without_comments.push_str(&pending_comma.take().unwrap());
        }

        if ch == '"' {
            in_string = true;
            without_comments.push(ch);
            continue;
        }

        if ch == ',' {
            pending_comma = Some(ch.to_string());
            continue;
        }

        if ch == '/' {
            match chars.peek() {
                Some('/') => {
                    chars.next();
                    while let Some(comment_ch) = chars.next() {
                        if comment_ch == '\n' {
                            without_comments.push('\n');
                            break;
                        }
                    }
                    continue;
                }
                Some('*') => {
                    chars.next();
                    let mut previous = '\0';
                    while let Some(comment_ch) = chars.next() {
                        if comment_ch == '\n' {
                            without_comments.push('\n');
                        }
                        if previous == '*' && comment_ch == '/' {
                            break;
                        }
                        previous = comment_ch;
                    }
                    continue;
                }
                _ => {}
            }
        }

        without_comments.push(ch);
    }

    if let Some(comma) = pending_comma {
        without_comments.push_str(&comma);
    }

    without_comments
}
