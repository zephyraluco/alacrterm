pub mod paths;
pub mod rel_path;
pub mod shell;

use std::{borrow::Cow, path::{Component, Path, PathBuf}};

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
