use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use itertools::Itertools;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cmp::Ordering;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::mem;
use std::path::StripPrefixError;
use std::sync::Arc;
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::LazyLock,
};

use crate::rel_path::RelPath;
use crate::rel_path::RelPathBuf;

/// Returns the path to the user's home directory.
pub fn home_dir() -> &'static PathBuf {
    static HOME_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    HOME_DIR.get_or_init(|| {
        if cfg!(any(test, feature = "test-support")) {
            if cfg!(target_os = "macos") {
                PathBuf::from("/Users/zed")
            } else if cfg!(target_os = "windows") {
                PathBuf::from("C:\\Users\\zed")
            } else {
                PathBuf::from("/home/zed")
            }
        } else {
            dirs::home_dir().expect("failed to determine home directory")
        }
    })
}

pub trait PathExt {
    /// Compacts a given file path by replacing the user's home directory
    /// prefix with a tilde (`~`).
    ///
    /// # Returns
    ///
    /// * A `PathBuf` containing the compacted file path. If the input path
    ///   does not have the user's home directory prefix, or if we are not on
    ///   Linux or macOS, the original path is returned unchanged.
    fn compact(&self) -> PathBuf;

    /// Returns a file's extension or, if the file is hidden, its name without the leading dot
    fn extension_or_hidden_file_name(&self) -> Option<&str>;

    fn try_from_bytes<'a>(bytes: &'a [u8]) -> anyhow::Result<Self>
    where
        Self: From<&'a Path>,
    {
        #[cfg(target_family = "wasm")]
        {
            std::str::from_utf8(bytes)
                .map(Path::new)
                .map(Into::into)
                .map_err(Into::into)
        }
        #[cfg(unix)]
        {
            use std::os::unix::prelude::OsStrExt;
            Ok(Self::from(Path::new(OsStr::from_bytes(bytes))))
        }
        #[cfg(windows)]
        {
            use anyhow::Context;
            use tendril::fmt::{Format, WTF8};
            WTF8::validate(bytes)
                .then(|| {
                    // Safety: bytes are valid WTF-8 sequence.
                    Self::from(Path::new(unsafe {
                        OsStr::from_encoded_bytes_unchecked(bytes)
                    }))
                })
                .with_context(|| format!("Invalid WTF-8 sequence: {bytes:?}"))
        }
    }

    /// Converts a local path to one that can be used inside of WSL.
    /// Returns `None` if the path cannot be converted into a WSL one (network share).
    fn local_to_wsl(&self) -> Option<PathBuf>;

    /// Returns a file's "full" joined collection of extensions, in the case where a file does not
    /// just have a singular extension but instead has multiple (e.g File.tar.gz, Component.stories.tsx)
    ///
    /// Will provide back the extensions joined together such as tar.gz or stories.tsx
    fn multiple_extensions(&self) -> Option<String>;

    /// Try to make a shell-safe representation of the path.
    #[cfg(not(target_family = "wasm"))]
    fn try_shell_safe(&self, shell_kind: crate::shell::ShellKind) -> anyhow::Result<String>;
}

impl<T: AsRef<Path>> PathExt for T {
    fn compact(&self) -> PathBuf {
        #[cfg(target_family = "wasm")]
        {
            self.as_ref().to_path_buf()
        }
        #[cfg(not(target_family = "wasm"))]
        if cfg!(any(target_os = "linux", target_os = "freebsd")) || cfg!(target_os = "macos") {
            match self.as_ref().strip_prefix(home_dir().as_path()) {
                Ok(relative_path) => {
                    let mut shortened_path = PathBuf::new();
                    shortened_path.push("~");
                    shortened_path.push(relative_path);
                    shortened_path
                }
                Err(_) => self.as_ref().to_path_buf(),
            }
        } else {
            self.as_ref().to_path_buf()
        }
    }

    fn extension_or_hidden_file_name(&self) -> Option<&str> {
        let path = self.as_ref();
        let file_name = path.file_name()?.to_str()?;
        if file_name.starts_with('.') {
            return file_name.strip_prefix('.');
        }

        path.extension()
            .and_then(|e| e.to_str())
            .or_else(|| path.file_stem()?.to_str())
    }

    fn local_to_wsl(&self) -> Option<PathBuf> {
        // quite sketchy to convert this back to path at the end, but a lot of functions only accept paths
        // todo: ideally rework them..?
        let mut new_path = std::ffi::OsString::new();
        for component in self.as_ref().components() {
            match component {
                std::path::Component::Prefix(prefix) => {
                    let drive_letter = prefix.as_os_str().to_string_lossy().to_lowercase();
                    let drive_letter = drive_letter.strip_suffix(':')?;

                    new_path.push(format!("/mnt/{}", drive_letter));
                }
                std::path::Component::RootDir => {}
                std::path::Component::CurDir => {
                    new_path.push("/.");
                }
                std::path::Component::ParentDir => {
                    new_path.push("/..");
                }
                std::path::Component::Normal(os_str) => {
                    new_path.push("/");
                    new_path.push(os_str);
                }
            }
        }

        Some(new_path.into())
    }

    fn multiple_extensions(&self) -> Option<String> {
        let path = self.as_ref();
        let file_name = path.file_name()?.to_str()?;

        let parts: Vec<&str> = file_name
            .split('.')
            // Skip the part with the file name extension
            .skip(1)
            .collect();

        if parts.len() < 2 {
            return None;
        }

        Some(parts.into_iter().join("."))
    }

    #[cfg(not(target_family = "wasm"))]
    fn try_shell_safe(&self, shell_kind: crate::shell::ShellKind) -> anyhow::Result<String> {
        use anyhow::Context;
        let path_str = self
            .as_ref()
            .to_str()
            .with_context(|| "Path contains invalid UTF-8")?;
        shell_kind
            .try_quote(path_str)
            .as_deref()
            .map(ToOwned::to_owned)
            .context("Failed to quote path")
    }
}

pub fn path_ends_with(base: &Path, suffix: &Path) -> bool {
    strip_path_suffix(base, suffix).is_some()
}

/// Case-insensitive ASCII comparison of a path component to a literal
/// folder name. macOS and Windows use case-insensitive filesystems by
/// default, so a path like `.ZED/settings.json` resolves to the same
/// inode as the lowercase form. A case-sensitive `==` check would miss
/// those and let a malicious settings author bypass classifiers with
/// unusual casing. Callers should restrict `name` to ASCII; for ASCII
/// inputs `eq_ignore_ascii_case` is safe and stable across platforms.
pub fn component_matches_ignore_ascii_case(component: &OsStr, name: &str) -> bool {
    component
        .to_str()
        .is_some_and(|s| s.eq_ignore_ascii_case(name))
}

pub fn strip_path_suffix<'a>(base: &'a Path, suffix: &Path) -> Option<&'a Path> {
    if let Some(remainder) = base
        .as_os_str()
        .as_encoded_bytes()
        .strip_suffix(suffix.as_os_str().as_encoded_bytes())
    {
        if remainder
            .last()
            .is_none_or(|last_byte| std::path::is_separator(*last_byte as char))
        {
            let os_str = unsafe {
                OsStr::from_encoded_bytes_unchecked(
                    &remainder[0..remainder.len().saturating_sub(1)],
                )
            };
            return Some(Path::new(os_str));
        }
    }
    None
}

/// In memory, this is identical to `Path`. On non-Windows conversions to this type are no-ops. On
/// windows, these conversions sanitize UNC paths by removing the `\\\\?\\` prefix.
#[derive(Eq, PartialEq, Hash, Ord, PartialOrd)]
#[repr(transparent)]
pub struct SanitizedPath(Path);

impl SanitizedPath {
    pub fn new<T: AsRef<Path> + ?Sized>(path: &T) -> &Self {
        #[cfg(not(target_os = "windows"))]
        return Self::unchecked_new(path.as_ref());

        #[cfg(target_os = "windows")]
        return Self::unchecked_new(dunce::simplified(path.as_ref()));
    }

    pub fn unchecked_new<T: AsRef<Path> + ?Sized>(path: &T) -> &Self {
        // safe because `Path` and `SanitizedPath` have the same repr and Drop impl
        unsafe { mem::transmute::<&Path, &Self>(path.as_ref()) }
    }

    pub fn from_arc(path: Arc<Path>) -> Arc<Self> {
        // safe because `Path` and `SanitizedPath` have the same repr and Drop impl
        #[cfg(not(target_os = "windows"))]
        return unsafe { mem::transmute::<Arc<Path>, Arc<Self>>(path) };

        #[cfg(target_os = "windows")]
        {
            let simplified = dunce::simplified(path.as_ref());
            if simplified == path.as_ref() {
                // safe because `Path` and `SanitizedPath` have the same repr and Drop impl
                unsafe { mem::transmute::<Arc<Path>, Arc<Self>>(path) }
            } else {
                Self::unchecked_new(simplified).into()
            }
        }
    }

    pub fn new_arc<T: AsRef<Path> + ?Sized>(path: &T) -> Arc<Self> {
        Self::new(path).into()
    }

    pub fn cast_arc(path: Arc<Self>) -> Arc<Path> {
        // safe because `Path` and `SanitizedPath` have the same repr and Drop impl
        unsafe { mem::transmute::<Arc<Self>, Arc<Path>>(path) }
    }

    pub fn cast_arc_ref(path: &Arc<Self>) -> &Arc<Path> {
        // safe because `Path` and `SanitizedPath` have the same repr and Drop impl
        unsafe { mem::transmute::<&Arc<Self>, &Arc<Path>>(path) }
    }

    pub fn starts_with(&self, prefix: &Self) -> bool {
        self.0.starts_with(&prefix.0)
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub fn file_name(&self) -> Option<&std::ffi::OsStr> {
        self.0.file_name()
    }

    pub fn extension(&self) -> Option<&std::ffi::OsStr> {
        self.0.extension()
    }

    pub fn join<P: AsRef<Path>>(&self, path: P) -> PathBuf {
        self.0.join(path)
    }

    pub fn parent(&self) -> Option<&Self> {
        self.0.parent().map(Self::unchecked_new)
    }

    pub fn strip_prefix(&self, base: &Self) -> Result<&Path, StripPrefixError> {
        self.0.strip_prefix(base.as_path())
    }

    pub fn to_str(&self) -> Option<&str> {
        self.0.to_str()
    }

    pub fn to_path_buf(&self) -> PathBuf {
        self.0.to_path_buf()
    }
}

impl std::fmt::Debug for SanitizedPath {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.0, formatter)
    }
}

impl Display for SanitizedPath {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.display())
    }
}

impl From<&SanitizedPath> for Arc<SanitizedPath> {
    fn from(sanitized_path: &SanitizedPath) -> Self {
        let path: Arc<Path> = sanitized_path.0.into();
        // safe because `Path` and `SanitizedPath` have the same repr and Drop impl
        unsafe { mem::transmute(path) }
    }
}

impl From<&SanitizedPath> for PathBuf {
    fn from(sanitized_path: &SanitizedPath) -> Self {
        sanitized_path.as_path().into()
    }
}

impl AsRef<Path> for SanitizedPath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathStyle {
    Posix,
    Windows,
}

impl PathStyle {
    #[cfg(target_os = "windows")]
    pub const fn local() -> Self {
        PathStyle::Windows
    }

    #[cfg(not(target_os = "windows"))]
    pub const fn local() -> Self {
        PathStyle::Posix
    }

    #[inline]
    pub fn primary_separator(&self) -> &'static str {
        match self {
            PathStyle::Posix => "/",
            PathStyle::Windows => "\\",
        }
    }

    pub fn separators(&self) -> &'static [&'static str] {
        match self {
            PathStyle::Posix => &["/"],
            PathStyle::Windows => &["\\", "/"],
        }
    }

    pub fn separators_ch(&self) -> &'static [char] {
        match self {
            PathStyle::Posix => &['/'],
            PathStyle::Windows => &['\\', '/'],
        }
    }

    pub fn is_absolute(&self, path_like: &str) -> bool {
        path_like.starts_with('/')
            || *self == PathStyle::Windows
                && (path_like.starts_with('\\')
                    || path_like
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphabetic())
                        && path_like[1..]
                            .strip_prefix(':')
                            .is_some_and(|path| path.starts_with('/') || path.starts_with('\\')))
    }

    pub fn is_windows(&self) -> bool {
        *self == PathStyle::Windows
    }

    pub fn is_posix(&self) -> bool {
        *self == PathStyle::Posix
    }

    pub fn join(self, left: impl AsRef<Path>, right: impl AsRef<Path>) -> Option<String> {
        let right = right.as_ref().to_str()?;
        if is_absolute(right, self) {
            return None;
        }
        let left = left.as_ref().to_str()?;
        if left.is_empty() {
            Some(right.into())
        } else {
            Some(format!(
                "{left}{}{right}",
                if left.ends_with(self.primary_separator()) {
                    ""
                } else {
                    self.primary_separator()
                }
            ))
        }
    }

    pub fn join_path(
        self,
        left: impl AsRef<Path>,
        right: impl AsRef<Path>,
    ) -> anyhow::Result<PathBuf> {
        let left = left
            .as_ref()
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Path contains invalid UTF-8"))?;
        let right = right.as_ref();
        let right_string = right
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Path contains invalid UTF-8"))?;
        let joined = self
            .join(left, right_string)
            .ok_or_else(|| anyhow::anyhow!("Path must be relative: {right:?}"))?;
        Ok(PathBuf::from(self.normalize(&joined)))
    }

    pub fn normalize(self, path_like: &str) -> String {
        match self {
            PathStyle::Windows => crate::normalize_path(Path::new(path_like))
                .to_string_lossy()
                .into_owned(),
            PathStyle::Posix => {
                let is_absolute = path_like.starts_with('/');
                let remainder = if is_absolute {
                    path_like.trim_start_matches('/')
                } else {
                    path_like
                };

                let mut components = Vec::new();
                for component in remainder.split(self.separators_ch()) {
                    match component {
                        "" | "." => {}
                        ".." => {
                            if components
                                .last()
                                .is_some_and(|component| *component != "..")
                            {
                                components.pop();
                            } else if !is_absolute {
                                components.push(component);
                            }
                        }
                        component => components.push(component),
                    }
                }

                let normalized = components.join(self.primary_separator());
                if is_absolute && normalized.is_empty() {
                    "/".to_string()
                } else if is_absolute {
                    format!("/{normalized}")
                } else {
                    normalized
                }
            }
        }
    }

    pub fn split(self, path_like: &str) -> (Option<&str>, &str) {
        let Some(pos) = path_like.rfind(self.primary_separator()) else {
            return (None, path_like);
        };
        let filename_start = pos + self.primary_separator().len();
        (
            Some(&path_like[..filename_start]),
            &path_like[filename_start..],
        )
    }

    pub fn strip_prefix<'a>(
        &self,
        child: &'a Path,
        parent: &'a Path,
    ) -> Option<std::borrow::Cow<'a, RelPath>> {
        let parent = parent.to_str()?;
        if parent.is_empty() {
            return RelPath::new(child, *self).ok();
        }
        let parent = self
            .separators()
            .iter()
            .find_map(|sep| parent.strip_suffix(sep))
            .unwrap_or(parent);
        let child = child.to_str()?;

        // Match behavior of std::path::Path, which is case-insensitive for drive letters (e.g., "C:" == "c:")
        let stripped = if self.is_windows()
            && child.as_bytes().get(1) == Some(&b':')
            && parent.as_bytes().get(1) == Some(&b':')
            && child.as_bytes()[0].eq_ignore_ascii_case(&parent.as_bytes()[0])
        {
            child[2..].strip_prefix(&parent[2..])?
        } else {
            child.strip_prefix(parent)?
        };
        if let Some(relative) = self
            .separators()
            .iter()
            .find_map(|sep| stripped.strip_prefix(sep))
        {
            RelPath::new(relative.as_ref(), *self).ok()
        } else if stripped.is_empty() {
            Some(Cow::Borrowed(RelPath::empty()))
        } else {
            None
        }
    }
}

#[derive(Debug, Clone)]
pub struct RemotePathBuf {
    style: PathStyle,
    string: String,
}

impl RemotePathBuf {
    pub fn new(string: String, style: PathStyle) -> Self {
        Self { style, string }
    }

    pub fn from_str(path: &str, style: PathStyle) -> Self {
        Self::new(path.to_string(), style)
    }

    pub fn path_style(&self) -> PathStyle {
        self.style
    }

    pub fn to_proto(self) -> String {
        self.string
    }
}

impl Display for RemotePathBuf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.string)
    }
}

pub fn is_absolute(path_like: &str, path_style: PathStyle) -> bool {
    path_like.starts_with('/')
        || path_style == PathStyle::Windows
            && (path_like.starts_with('\\')
                || path_like
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic())
                    && path_like[1..]
                        .strip_prefix(':')
                        .is_some_and(|path| path.starts_with('/') || path.starts_with('\\')))
}

#[derive(Debug, PartialEq)]
#[non_exhaustive]
pub struct NormalizeError;

impl Error for NormalizeError {}

impl std::fmt::Display for NormalizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("parent reference `..` points outside of base directory")
    }
}

/// Copied from stdlib where it's unstable.
///
/// Normalize a path, including `..` without traversing the filesystem.
///
/// Returns an error if normalization would leave leading `..` components.
///
/// <div class="warning">
///
/// This function always resolves `..` to the "lexical" parent.
/// That is "a/b/../c" will always resolve to `a/c` which can change the meaning of the path.
/// In particular, `a/c` and `a/b/../c` are distinct on many systems because `b` may be a symbolic link, so its parent isn't `a`.
///
/// </div>
///
/// [`path::absolute`](absolute) is an alternative that preserves `..`.
/// Or [`Path::canonicalize`] can be used to resolve any `..` by querying the filesystem.
pub fn normalize_lexically(path: &Path) -> Result<PathBuf, NormalizeError> {
    use std::path::Component;

    let mut lexical = PathBuf::new();
    let mut iter = path.components().peekable();

    // Find the root, if any, and add it to the lexical path.
    // Here we treat the Windows path "C:\" as a single "root" even though
    // `components` splits it into two: (Prefix, RootDir).
    let root = match iter.peek() {
        Some(Component::ParentDir) => return Err(NormalizeError),
        Some(p @ Component::RootDir) | Some(p @ Component::CurDir) => {
            lexical.push(p);
            iter.next();
            lexical.as_os_str().len()
        }
        Some(Component::Prefix(prefix)) => {
            lexical.push(prefix.as_os_str());
            iter.next();
            if let Some(p @ Component::RootDir) = iter.peek() {
                lexical.push(p);
                iter.next();
            }
            lexical.as_os_str().len()
        }
        None => return Ok(PathBuf::new()),
        Some(Component::Normal(_)) => 0,
    };

    for component in iter {
        match component {
            Component::RootDir => unreachable!(),
            Component::Prefix(_) => return Err(NormalizeError),
            Component::CurDir => continue,
            Component::ParentDir => {
                // It's an error if ParentDir causes us to go above the "root".
                if lexical.as_os_str().len() == root {
                    return Err(NormalizeError);
                } else {
                    lexical.pop();
                }
            }
            Component::Normal(path) => lexical.push(path),
        }
    }
    Ok(lexical)
}

/// Insert `path` into a set of "subtree" grants, keeping the set minimal.
///
/// A subtree grant covers a path and all of its descendants. Insertion is a
/// no-op when `path` is already covered by an existing (equal-or-broader)
/// entry; otherwise `path` is added and any now-subsumed descendant entries
/// are pruned. Containment is purely lexical (component-wise `starts_with`),
/// so callers should normalize paths (e.g. via [`normalize_lexically`]) before
/// inserting, otherwise `..` components can defeat the containment checks.
pub fn insert_subtree(subtrees: &mut Vec<PathBuf>, path: PathBuf) {
    if subtrees.iter().any(|existing| path.starts_with(existing)) {
        return;
    }
    subtrees.retain(|existing| !existing.starts_with(&path));
    subtrees.push(path);
}

/// Whether `path` sits under (or exactly equals) any of the given subtree
/// grants. As with [`insert_subtree`], containment is purely lexical, so
/// callers should pass normalized paths.
pub fn path_within_subtree<'a>(path: &Path, mut subtrees: impl Iterator<Item = &'a Path>) -> bool {
    subtrees.any(|granted| path.starts_with(granted))
}

/// A delimiter to use in `path_query:row_number:column_number` strings parsing.
pub const FILE_ROW_COLUMN_DELIMITER: char = ':';

const ROW_COL_CAPTURE_REGEX: &str = r"(?xs)
    ([^\(]+)\:(?:
        \((\d+)[,:](\d+)\) # filename:(row,column), filename:(row:column)
        |
        \((\d+)\)()     # filename:(row)
    )
    |
    ([^\(]+)(?:
        \((\d+)[,:](\d+)\) # filename(row,column), filename(row:column)
        |
        \((\d+)\)()     # filename(row)
    )
    \:*$
    |
    (.+?)(?:
        \:+(\d+)\:(\d+)\:*$  # filename:row:column
        |
        \:+(\d+)\:*()$       # filename:row
        |
        \:+()()$
    )";

/// A representation of a path-like string with optional row and column numbers.
/// Matching values example: `te`, `test.rs:22`, `te:22:5`, `test.c(22)`, `test.c(22,5)`etc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct PathWithPosition {
    pub path: PathBuf,
    pub row: Option<u32>,
    // Absent if row is absent.
    pub column: Option<u32>,
}

impl PathWithPosition {
    /// Returns a PathWithPosition from a path.
    pub fn from_path(path: PathBuf) -> Self {
        Self {
            path,
            row: None,
            column: None,
        }
    }

    /// Parses a string that possibly has `:row:column` or `(row, column)` suffix.
    /// Parenthesis format is used by [MSBuild](https://learn.microsoft.com/en-us/visualstudio/msbuild/msbuild-diagnostic-format-for-tasks) compatible tools
    /// Ignores trailing `:`s, so `test.rs:22:` is parsed as `test.rs:22`.
    /// If the suffix parsing fails, the whole string is parsed as a path.
    ///
    /// Be mindful that `test_file:10:1:` is a valid posix filename.
    /// `PathWithPosition` class assumes that the ending position-like suffix is **not** part of the filename.
    ///
    /// # Examples
    ///
    /// ```
    /// # use util::paths::PathWithPosition;
    /// # use std::path::PathBuf;
    /// assert_eq!(PathWithPosition::parse_str("test_file"), PathWithPosition {
    ///     path: PathBuf::from("test_file"),
    ///     row: None,
    ///     column: None,
    /// });
    /// assert_eq!(PathWithPosition::parse_str("test_file:10"), PathWithPosition {
    ///     path: PathBuf::from("test_file"),
    ///     row: Some(10),
    ///     column: None,
    /// });
    /// assert_eq!(PathWithPosition::parse_str("test_file.rs"), PathWithPosition {
    ///     path: PathBuf::from("test_file.rs"),
    ///     row: None,
    ///     column: None,
    /// });
    /// assert_eq!(PathWithPosition::parse_str("test_file.rs:1"), PathWithPosition {
    ///     path: PathBuf::from("test_file.rs"),
    ///     row: Some(1),
    ///     column: None,
    /// });
    /// assert_eq!(PathWithPosition::parse_str("test_file.rs:1:2"), PathWithPosition {
    ///     path: PathBuf::from("test_file.rs"),
    ///     row: Some(1),
    ///     column: Some(2),
    /// });
    /// ```
    ///
    /// # Expected parsing results when encounter ill-formatted inputs.
    /// ```
    /// # use util::paths::PathWithPosition;
    /// # use std::path::PathBuf;
    /// assert_eq!(PathWithPosition::parse_str("test_file.rs:a"), PathWithPosition {
    ///     path: PathBuf::from("test_file.rs:a"),
    ///     row: None,
    ///     column: None,
    /// });
    /// assert_eq!(PathWithPosition::parse_str("test_file.rs:a:b"), PathWithPosition {
    ///     path: PathBuf::from("test_file.rs:a:b"),
    ///     row: None,
    ///     column: None,
    /// });
    /// assert_eq!(PathWithPosition::parse_str("test_file.rs"), PathWithPosition {
    ///     path: PathBuf::from("test_file.rs"),
    ///     row: None,
    ///     column: None,
    /// });
    /// assert_eq!(PathWithPosition::parse_str("test_file.rs::1"), PathWithPosition {
    ///     path: PathBuf::from("test_file.rs"),
    ///     row: Some(1),
    ///     column: None,
    /// });
    /// assert_eq!(PathWithPosition::parse_str("test_file.rs:1::"), PathWithPosition {
    ///     path: PathBuf::from("test_file.rs"),
    ///     row: Some(1),
    ///     column: None,
    /// });
    /// assert_eq!(PathWithPosition::parse_str("test_file.rs::1:2"), PathWithPosition {
    ///     path: PathBuf::from("test_file.rs"),
    ///     row: Some(1),
    ///     column: Some(2),
    /// });
    /// assert_eq!(PathWithPosition::parse_str("test_file.rs:1::2"), PathWithPosition {
    ///     path: PathBuf::from("test_file.rs:1"),
    ///     row: Some(2),
    ///     column: None,
    /// });
    /// assert_eq!(PathWithPosition::parse_str("test_file.rs:1:2:3"), PathWithPosition {
    ///     path: PathBuf::from("test_file.rs:1"),
    ///     row: Some(2),
    ///     column: Some(3),
    /// });
    /// ```
    pub fn parse_str(s: &str) -> Self {
        let trimmed = s.trim();
        let path = Path::new(trimmed);
        let Some(maybe_file_name_with_row_col) = path.file_name().unwrap_or_default().to_str()
        else {
            return Self {
                path: Path::new(s).to_path_buf(),
                row: None,
                column: None,
            };
        };
        if maybe_file_name_with_row_col.is_empty() {
            return Self {
                path: Path::new(s).to_path_buf(),
                row: None,
                column: None,
            };
        }

        // Let's avoid repeated init cost on this. It is subject to thread contention, but
        // so far this code isn't called from multiple hot paths. Getting contention here
        // in the future seems unlikely.
        static SUFFIX_RE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(ROW_COL_CAPTURE_REGEX).unwrap());
        match SUFFIX_RE
            .captures(maybe_file_name_with_row_col)
            .map(|caps| caps.extract())
        {
            Some((_, [file_name, maybe_row, maybe_column])) => {
                let row = maybe_row.parse::<u32>().ok();
                let column = maybe_column.parse::<u32>().ok();

                let (_, suffix) = trimmed.split_once(file_name).unwrap();
                let path_without_suffix = &trimmed[..trimmed.len() - suffix.len()];

                Self {
                    path: Path::new(path_without_suffix).to_path_buf(),
                    row,
                    column,
                }
            }
            None => {
                // The `ROW_COL_CAPTURE_REGEX` deals with separated digits only,
                // but in reality there could be `foo/bar.py:22:in` inputs which we want to match too.
                // The regex mentioned is not very extendable with "digit or random string" checks, so do this here instead.
                let delimiter = ':';
                let mut path_parts = s
                    .rsplitn(3, delimiter)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .fuse();
                let mut path_string = path_parts.next().expect("rsplitn should have the rest of the string as its last parameter that we reversed").to_owned();
                let mut row = None;
                let mut column = None;
                if let Some(maybe_row) = path_parts.next() {
                    if let Ok(parsed_row) = maybe_row.parse::<u32>() {
                        row = Some(parsed_row);
                        if let Some(parsed_column) = path_parts
                            .next()
                            .and_then(|maybe_col| maybe_col.parse::<u32>().ok())
                        {
                            column = Some(parsed_column);
                        }
                    } else {
                        path_string.push(delimiter);
                        path_string.push_str(maybe_row);
                    }
                }
                for split in path_parts {
                    path_string.push(delimiter);
                    path_string.push_str(split);
                }

                Self {
                    path: PathBuf::from(path_string),
                    row,
                    column,
                }
            }
        }
    }

    pub fn map_path<E>(
        self,
        mapping: impl FnOnce(PathBuf) -> Result<PathBuf, E>,
    ) -> Result<PathWithPosition, E> {
        Ok(PathWithPosition {
            path: mapping(self.path)?,
            row: self.row,
            column: self.column,
        })
    }

    pub fn to_string(&self, path_to_string: &dyn Fn(&PathBuf) -> String) -> String {
        let path_string = path_to_string(&self.path);
        if let Some(row) = self.row {
            if let Some(column) = self.column {
                format!("{path_string}:{row}:{column}")
            } else {
                format!("{path_string}:{row}")
            }
        } else {
            path_string
        }
    }
}

#[derive(Clone)]
pub struct PathMatcher {
    sources: Vec<(String, RelPathBuf, /*trailing separator*/ bool)>,
    glob: GlobSet,
    path_style: PathStyle,
}

impl std::fmt::Debug for PathMatcher {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PathMatcher")
            .field("sources", &self.sources)
            .field("path_style", &self.path_style)
            .finish()
    }
}

impl PartialEq for PathMatcher {
    fn eq(&self, other: &Self) -> bool {
        self.sources.eq(&other.sources)
    }
}

impl Eq for PathMatcher {}

impl PathMatcher {
    pub fn new(
        globs: impl IntoIterator<Item = impl AsRef<str>>,
        path_style: PathStyle,
    ) -> Result<Self, globset::Error> {
        let globs = globs
            .into_iter()
            .map(|as_str| {
                GlobBuilder::new(as_str.as_ref())
                    .backslash_escape(path_style.is_posix())
                    .build()
            })
            .collect::<Result<Vec<_>, _>>()?;
        let sources = globs
            .iter()
            .filter_map(|glob| {
                let glob = glob.glob();
                Some((
                    glob.to_string(),
                    RelPath::new(&glob.as_ref(), path_style)
                        .ok()
                        .map(std::borrow::Cow::into_owned)?,
                    glob.ends_with(path_style.separators_ch()),
                ))
            })
            .collect();
        let mut glob_builder = GlobSetBuilder::new();
        for single_glob in globs {
            glob_builder.add(single_glob);
        }
        let glob = glob_builder.build()?;
        Ok(PathMatcher {
            glob,
            sources,
            path_style,
        })
    }

    pub fn sources(&self) -> impl Iterator<Item = &str> + Clone {
        self.sources.iter().map(|(source, ..)| source.as_str())
    }

    pub fn is_match<P: AsRef<RelPath>>(&self, other: P) -> bool {
        let other = other.as_ref();
        if self
            .sources
            .iter()
            .any(|(_, source, _)| other.starts_with(source) || other.ends_with(source))
        {
            return true;
        }
        let other_path = other.display(self.path_style);

        if self.glob.is_match(&*other_path) {
            return true;
        }

        self.glob
            .is_match(other_path.into_owned() + self.path_style.primary_separator())
    }

    pub fn is_match_std_path<P: AsRef<Path>>(&self, other: P) -> bool {
        let other = other.as_ref();
        if self.sources.iter().any(|(_, source, _)| {
            other.starts_with(source.as_std_path()) || other.ends_with(source.as_std_path())
        }) {
            return true;
        }
        self.glob.is_match(other)
    }
}

impl Default for PathMatcher {
    fn default() -> Self {
        Self {
            path_style: PathStyle::local(),
            glob: GlobSet::empty(),
            sources: vec![],
        }
    }
}

/// Compares two sequences of consecutive digits for natural sorting.
///
/// This function is a core component of natural sorting that handles numeric comparison
/// in a way that feels natural to humans. It extracts and compares consecutive digit
/// sequences from two iterators, handling various cases like leading zeros and very large numbers.
///
/// # Behavior
///
/// The function implements the following comparison rules:
/// 1. Different numeric values: Compares by actual numeric value (e.g., "2" < "10")
/// 2. Leading zeros: When values are equal, longer sequence wins (e.g., "002" > "2")
/// 3. Large numbers: Falls back to string comparison for numbers that would overflow u128
///
/// # Examples
///
/// ```text
/// "1" vs "2"      -> Less       (different values)
/// "2" vs "10"     -> Less       (numeric comparison)
/// "002" vs "2"    -> Greater    (leading zeros)
/// "10" vs "010"   -> Less       (leading zeros)
/// "999..." vs "1000..." -> Less (large number comparison)
/// ```
///
/// # Implementation Details
///
/// 1. Extracts consecutive digits into strings
/// 2. Compares sequence lengths for leading zero handling
/// 3. For equal lengths, compares digit by digit
/// 4. For different lengths:
///    - Attempts numeric comparison first (for numbers up to 2^128 - 1)
///    - Falls back to string comparison if numbers would overflow
///
/// The function advances both iterators past their respective numeric sequences,
/// regardless of the comparison result.
fn compare_numeric_segments<I>(
    a_iter: &mut std::iter::Peekable<I>,
    b_iter: &mut std::iter::Peekable<I>,
) -> Ordering
where
    I: Iterator<Item = char>,
{
    // Collect all consecutive digits into strings
    let mut a_num_str = String::new();
    let mut b_num_str = String::new();

    while let Some(&c) = a_iter.peek() {
        if !c.is_ascii_digit() {
            break;
        }

        a_num_str.push(c);
        a_iter.next();
    }

    while let Some(&c) = b_iter.peek() {
        if !c.is_ascii_digit() {
            break;
        }

        b_num_str.push(c);
        b_iter.next();
    }

    // First compare lengths (handle leading zeros)
    match a_num_str.len().cmp(&b_num_str.len()) {
        Ordering::Equal => {
            // Same length, compare digit by digit
            match a_num_str.cmp(&b_num_str) {
                Ordering::Equal => Ordering::Equal,
                ordering => ordering,
            }
        }

        // Different lengths but same value means leading zeros
        ordering => {
            // Try parsing as numbers first
            if let (Ok(a_val), Ok(b_val)) = (a_num_str.parse::<u128>(), b_num_str.parse::<u128>()) {
                match a_val.cmp(&b_val) {
                    Ordering::Equal => ordering, // Same value, longer one is greater (leading zeros)
                    ord => ord,
                }
            } else {
                // If parsing fails (overflow), compare as strings
                a_num_str.cmp(&b_num_str)
            }
        }
    }
}

/// Performs natural sorting comparison between two strings.
///
/// Natural sorting is an ordering that handles numeric sequences in a way that matches human expectations.
/// For example, "file2" comes before "file10" (unlike standard lexicographic sorting).
///
/// # Characteristics
///
/// * Case-sensitive with lowercase priority: When comparing same letters, lowercase comes before uppercase
/// * Numbers are compared by numeric value, not character by character
/// * Leading zeros affect ordering when numeric values are equal
/// * Can handle numbers larger than u128::MAX (falls back to string comparison)
/// * When strings are equal case-insensitively, lowercase is prioritized (lowercase < uppercase)
///
/// # Algorithm
///
/// The function works by:
/// 1. Processing strings character by character in a case-insensitive manner
/// 2. When encountering digits, treating consecutive digits as a single number
/// 3. Comparing numbers by their numeric value rather than lexicographically
/// 4. For non-numeric characters, using case-insensitive comparison
/// 5. If everything is equal case-insensitively, using case-sensitive comparison as final tie-breaker
pub fn natural_sort(a: &str, b: &str) -> Ordering {
    let mut a_iter = a.chars().peekable();
    let mut b_iter = b.chars().peekable();

    loop {
        match (a_iter.peek(), b_iter.peek()) {
            (None, None) => {
                return b.cmp(a);
            }
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(&a_char), Some(&b_char)) => {
                if a_char.is_ascii_digit() && b_char.is_ascii_digit() {
                    match compare_numeric_segments(&mut a_iter, &mut b_iter) {
                        Ordering::Equal => continue,
                        ordering => return ordering,
                    }
                } else {
                    match a_char
                        .to_ascii_lowercase()
                        .cmp(&b_char.to_ascii_lowercase())
                    {
                        Ordering::Equal => {
                            a_iter.next();
                            b_iter.next();
                        }
                        ordering => return ordering,
                    }
                }
            }
        }
    }
}

/// Case-insensitive natural sort without applying the final lowercase/uppercase tie-breaker.
/// This is useful when comparing individual path components where we want to keep walking
/// deeper components before deciding on casing.
fn natural_sort_no_tiebreak(a: &str, b: &str) -> Ordering {
    if a.eq_ignore_ascii_case(b) {
        Ordering::Equal
    } else {
        natural_sort(a, b)
    }
}

fn stem_and_extension(filename: &str) -> (Option<&str>, Option<&str>) {
    if filename.is_empty() {
        return (None, None);
    }

    match filename.rsplit_once('.') {
        // Case 1: No dot was found. The entire name is the stem.
        None => (Some(filename), None),

        // Case 2: A dot was found.
        Some((before, after)) => {
            // This is the crucial check for dotfiles like ".bashrc".
            // If `before` is empty, the dot was the first character.
            // In that case, we revert to the "whole name is the stem" logic.
            if before.is_empty() {
                (Some(filename), None)
            } else {
                // Otherwise, we have a standard stem and extension.
                (Some(before), Some(after))
            }
        }
    }
}

/// Controls the lexicographic sorting of file and folder names.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum SortOrder {
    /// Case-insensitive natural sort with lowercase preferred in ties.
    /// Numbers in file names are compared by value (e.g., `file2` before `file10`).
    #[default]
    Default,
    /// Uppercase names are grouped before lowercase names, with case-insensitive
    /// natural sort within each group. Dot-prefixed names sort before both groups.
    Upper,
    /// Lowercase names are grouped before uppercase names, with case-insensitive
    /// natural sort within each group. Dot-prefixed names sort before both groups.
    Lower,
    /// Pure Unicode codepoint comparison. No case folding, no natural number sorting.
    /// Uppercase ASCII sorts before lowercase. Accented characters sort after ASCII.
    Unicode,
}

/// Controls how files and directories are ordered relative to each other.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum SortMode {
    /// Directories are listed before files at each level.
    #[default]
    DirectoriesFirst,
    /// Files and directories are interleaved alphabetically.
    Mixed,
    /// Files are listed before directories at each level.
    FilesFirst,
}

fn case_group_key(name: &str, order: SortOrder) -> u8 {
    let first = match name.chars().next() {
        Some(c) => c,
        None => return 0,
    };
    match order {
        SortOrder::Upper => {
            if first.is_lowercase() {
                1
            } else {
                0
            }
        }
        SortOrder::Lower => {
            if first.is_uppercase() {
                1
            } else {
                0
            }
        }
        _ => 0,
    }
}

fn compare_strings(a: &str, b: &str, order: SortOrder) -> Ordering {
    match order {
        SortOrder::Unicode => a.cmp(b),
        _ => natural_sort(a, b),
    }
}

fn compare_strings_no_tiebreak(a: &str, b: &str, order: SortOrder) -> Ordering {
    match order {
        SortOrder::Unicode => a.cmp(b),
        _ => natural_sort_no_tiebreak(a, b),
    }
}

pub fn compare_rel_paths(
    (path_a, a_is_file): (&RelPath, bool),
    (path_b, b_is_file): (&RelPath, bool),
) -> Ordering {
    compare_rel_paths_by(
        (path_a, a_is_file),
        (path_b, b_is_file),
        SortMode::DirectoriesFirst,
        SortOrder::Default,
    )
}

pub fn compare_rel_paths_by(
    (path_a, a_is_file): (&RelPath, bool),
    (path_b, b_is_file): (&RelPath, bool),
    mode: SortMode,
    order: SortOrder,
) -> Ordering {
    let needs_final_tiebreak =
        mode != SortMode::DirectoriesFirst && !(std::ptr::eq(path_a, path_b) || path_a == path_b);

    let mut components_a = path_a.components();
    let mut components_b = path_b.components();

    loop {
        match (components_a.next(), components_b.next()) {
            (Some(component_a), Some(component_b)) => {
                let a_leaf_file = a_is_file && components_a.rest().is_empty();
                let b_leaf_file = b_is_file && components_b.rest().is_empty();

                let file_dir_ordering = match mode {
                    SortMode::DirectoriesFirst => a_leaf_file.cmp(&b_leaf_file),
                    SortMode::FilesFirst => b_leaf_file.cmp(&a_leaf_file),
                    SortMode::Mixed => Ordering::Equal,
                };

                if !file_dir_ordering.is_eq() {
                    return file_dir_ordering;
                }

                let (a_stem, a_ext) = a_leaf_file
                    .then(|| stem_and_extension(component_a))
                    .unwrap_or_default();
                let (b_stem, b_ext) = b_leaf_file
                    .then(|| stem_and_extension(component_b))
                    .unwrap_or_default();
                let a_key = if a_leaf_file {
                    a_stem
                } else {
                    Some(component_a)
                };
                let b_key = if b_leaf_file {
                    b_stem
                } else {
                    Some(component_b)
                };

                let ordering = match (a_key, b_key) {
                    (Some(a), Some(b)) => {
                        let name_cmp = case_group_key(a, order)
                            .cmp(&case_group_key(b, order))
                            .then_with(|| match mode {
                                SortMode::DirectoriesFirst => compare_strings(a, b, order),
                                _ => compare_strings_no_tiebreak(a, b, order),
                            });

                        let name_cmp = if mode == SortMode::Mixed {
                            name_cmp.then_with(|| match (a_leaf_file, b_leaf_file) {
                                (true, false) if a.eq_ignore_ascii_case(b) => Ordering::Greater,
                                (false, true) if a.eq_ignore_ascii_case(b) => Ordering::Less,
                                _ => Ordering::Equal,
                            })
                        } else {
                            name_cmp
                        };

                        name_cmp.then_with(|| {
                            if a_leaf_file && b_leaf_file {
                                match order {
                                    SortOrder::Unicode => {
                                        a_ext.unwrap_or_default().cmp(b_ext.unwrap_or_default())
                                    }
                                    _ => {
                                        let a_ext_str = a_ext.unwrap_or_default().to_lowercase();
                                        let b_ext_str = b_ext.unwrap_or_default().to_lowercase();
                                        a_ext_str.cmp(&b_ext_str)
                                    }
                                }
                            } else {
                                Ordering::Equal
                            }
                        })
                    }
                    (Some(_), None) => Ordering::Greater,
                    (None, Some(_)) => Ordering::Less,
                    (None, None) => Ordering::Equal,
                };

                if !ordering.is_eq() {
                    return ordering;
                }
            }
            (Some(_), None) => return Ordering::Greater,
            (None, Some(_)) => return Ordering::Less,
            (None, None) => {
                if needs_final_tiebreak {
                    return compare_strings(path_a.as_unix_str(), path_b.as_unix_str(), order);
                }
                return Ordering::Equal;
            }
        }
    }
}

pub fn compare_paths(
    (path_a, a_is_file): (&Path, bool),
    (path_b, b_is_file): (&Path, bool),
) -> Ordering {
    let mut components_a = path_a.components().peekable();
    let mut components_b = path_b.components().peekable();

    loop {
        match (components_a.next(), components_b.next()) {
            (Some(component_a), Some(component_b)) => {
                let a_is_file = components_a.peek().is_none() && a_is_file;
                let b_is_file = components_b.peek().is_none() && b_is_file;

                let ordering = a_is_file.cmp(&b_is_file).then_with(|| {
                    let path_a = Path::new(component_a.as_os_str());
                    let path_string_a = if a_is_file {
                        path_a.file_stem()
                    } else {
                        path_a.file_name()
                    }
                    .map(|s| s.to_string_lossy());

                    let path_b = Path::new(component_b.as_os_str());
                    let path_string_b = if b_is_file {
                        path_b.file_stem()
                    } else {
                        path_b.file_name()
                    }
                    .map(|s| s.to_string_lossy());

                    let compare_components = match (path_string_a, path_string_b) {
                        (Some(a), Some(b)) => natural_sort(&a, &b),
                        (Some(_), None) => Ordering::Greater,
                        (None, Some(_)) => Ordering::Less,
                        (None, None) => Ordering::Equal,
                    };

                    compare_components.then_with(|| {
                        if a_is_file && b_is_file {
                            let ext_a = path_a.extension().unwrap_or_default();
                            let ext_b = path_b.extension().unwrap_or_default();
                            ext_a.cmp(ext_b)
                        } else {
                            Ordering::Equal
                        }
                    })
                });

                if !ordering.is_eq() {
                    return ordering;
                }
            }
            (Some(_), None) => break Ordering::Greater,
            (None, Some(_)) => break Ordering::Less,
            (None, None) => break Ordering::Equal,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WslPath {
    pub distro: String,

    // the reason this is an OsString and not any of the path types is that it needs to
    // represent a unix path (with '/' separators) on windows. `from_path` does this by
    // manually constructing it from the path components of a given windows path.
    pub path: std::ffi::OsString,
}

impl WslPath {
    pub fn from_path<P: AsRef<Path>>(path: P) -> Option<WslPath> {
        if cfg!(not(target_os = "windows")) {
            return None;
        }
        use std::{
            ffi::OsString,
            path::{Component, Prefix},
        };

        let mut components = path.as_ref().components();
        let Some(Component::Prefix(prefix)) = components.next() else {
            return None;
        };
        let (server, distro) = match prefix.kind() {
            Prefix::UNC(server, distro) => (server, distro),
            Prefix::VerbatimUNC(server, distro) => (server, distro),
            _ => return None,
        };
        let Some(Component::RootDir) = components.next() else {
            return None;
        };

        let server_str = server.to_string_lossy();
        if server_str == "wsl.localhost" || server_str == "wsl$" {
            let mut result = OsString::from("");
            for c in components {
                use Component::*;
                match c {
                    Prefix(p) => unreachable!("got {p:?}, but already stripped prefix"),
                    RootDir => unreachable!("got root dir, but already stripped root"),
                    CurDir => continue,
                    ParentDir => result.push("/.."),
                    Normal(s) => {
                        result.push("/");
                        result.push(s);
                    }
                }
            }
            if result.is_empty() {
                result.push("/");
            }
            Some(WslPath {
                distro: distro.to_string_lossy().to_string(),
                path: result,
            })
        } else {
            None
        }
    }
}

pub trait UrlExt {
    /// A version of `url::Url::to_file_path` that does platform handling based on the provided `PathStyle` instead of the host platform.
    ///
    /// Prefer using this over `url::Url::to_file_path` when you need to handle paths in a cross-platform way as is the case for remoting interactions.
    fn to_file_path_ext(&self, path_style: PathStyle) -> Result<PathBuf, ()>;
}

impl UrlExt for url::Url {
    // Copied from `url::Url::to_file_path`, but the `cfg` handling is replaced with runtime branching on `PathStyle`
    fn to_file_path_ext(&self, source_path_style: PathStyle) -> Result<PathBuf, ()> {
        if let Some(segments) = self.path_segments() {
            let host = match self.host() {
                None | Some(url::Host::Domain("localhost")) => None,
                Some(_) if source_path_style.is_windows() && self.scheme() == "file" => {
                    self.host_str()
                }
                _ => return Err(()),
            };

            let str_len = self.as_str().len();
            let estimated_capacity = if source_path_style.is_windows() {
                // remove scheme: - has possible \\ for hostname
                str_len.saturating_sub(self.scheme().len() + 1)
            } else {
                // remove scheme://
                str_len.saturating_sub(self.scheme().len() + 3)
            };
            return match source_path_style {
                PathStyle::Posix => {
                    file_url_segments_to_pathbuf_posix(estimated_capacity, host, segments)
                }
                PathStyle::Windows => {
                    file_url_segments_to_pathbuf_windows(estimated_capacity, host, segments)
                }
            };
        }

        fn file_url_segments_to_pathbuf_posix(
            estimated_capacity: usize,
            host: Option<&str>,
            segments: std::str::Split<'_, char>,
        ) -> Result<PathBuf, ()> {
            use percent_encoding::percent_decode;

            if host.is_some() {
                return Err(());
            }

            let mut bytes = Vec::new();
            bytes.try_reserve(estimated_capacity).map_err(|_| ())?;

            for segment in segments {
                bytes.push(b'/');
                bytes.extend(percent_decode(segment.as_bytes()));
            }

            // A windows drive letter must end with a slash.
            if bytes.len() > 2
                && bytes[bytes.len() - 2].is_ascii_alphabetic()
                && matches!(bytes[bytes.len() - 1], b':' | b'|')
            {
                bytes.push(b'/');
            }

            let path = String::from_utf8(bytes).map_err(|_| ())?;
            debug_assert!(
                PathStyle::Posix.is_absolute(&path),
                "to_file_path() failed to produce an absolute Path"
            );

            Ok(PathBuf::from(path))
        }

        fn file_url_segments_to_pathbuf_windows(
            estimated_capacity: usize,
            host: Option<&str>,
            mut segments: std::str::Split<'_, char>,
        ) -> Result<PathBuf, ()> {
            use percent_encoding::percent_decode_str;
            let mut string = String::new();
            string.try_reserve(estimated_capacity).map_err(|_| ())?;
            if let Some(host) = host {
                string.push_str(r"\\");
                string.push_str(host);
            } else {
                let first = segments.next().ok_or(())?;

                match first.len() {
                    2 => {
                        if !first.starts_with(|c| char::is_ascii_alphabetic(&c))
                            || first.as_bytes()[1] != b':'
                        {
                            return Err(());
                        }

                        string.push_str(first);
                    }

                    4 => {
                        if !first.starts_with(|c| char::is_ascii_alphabetic(&c)) {
                            return Err(());
                        }
                        let bytes = first.as_bytes();
                        if bytes[1] != b'%'
                            || bytes[2] != b'3'
                            || (bytes[3] != b'a' && bytes[3] != b'A')
                        {
                            return Err(());
                        }

                        string.push_str(&first[0..1]);
                        string.push(':');
                    }

                    _ => return Err(()),
                }
            };

            for segment in segments {
                string.push('\\');

                // Currently non-unicode windows paths cannot be represented
                match percent_decode_str(segment).decode_utf8() {
                    Ok(s) => string.push_str(&s),
                    Err(..) => return Err(()),
                }
            }
            // ensure our estimated capacity was good
            if cfg!(test) {
                debug_assert!(
                    string.len() <= estimated_capacity,
                    "len: {}, capacity: {}",
                    string.len(),
                    estimated_capacity
                );
            }
            debug_assert!(
                PathStyle::Windows.is_absolute(&string),
                "to_file_path() failed to produce an absolute Path"
            );
            let path = PathBuf::from(string);
            Ok(path)
        }
        Err(())
    }
}

#[cfg(test)]
mod tests {
    use crate::rel_path::rel_path;

    use super::*;

    #[test]
    fn test_join_path_uses_path_style_separator() {
        let posix_path = PathStyle::Posix
            .join_path(Path::new("/home/user/dev"), "worktrees")
            .unwrap();
        let windows_path = PathStyle::Windows
            .join_path(Path::new("C:\\Users\\user\\dev"), "worktrees")
            .unwrap();

        assert_eq!(posix_path, PathBuf::from("/home/user/dev/worktrees"));
        assert_eq!(
            windows_path.to_string_lossy(),
            "C:\\Users\\user\\dev\\worktrees"
        );
    }

    #[test]
    fn test_normalize_uses_path_style_separator() {
        assert_eq!(
            PathStyle::Posix.normalize("/home/user/dev/../worktrees/./zed"),
            "/home/user/worktrees/zed"
        );
        assert_eq!(
            PathStyle::Windows.normalize("C:\\Users\\user\\dev\\worktrees"),
            "C:\\Users\\user\\dev\\worktrees"
        );
    }

    fn rel_path_entry(path: &'static str, is_file: bool) -> (&'static RelPath, bool) {
        (RelPath::unix(path).unwrap(), is_file)
    }

    fn sorted_rel_paths(
        mut paths: Vec<(&'static RelPath, bool)>,
        mode: SortMode,
        order: SortOrder,
    ) -> Vec<(&'static RelPath, bool)> {
        paths.sort_by(|&a, &b| compare_rel_paths_by(a, b, mode, order));
        paths
    }

    #[test]
    fn test_multiple_extensions() {
        // No extensions
        let path = Path::new("/a/b/c/file_name");
        assert_eq!(path.multiple_extensions(), None);

        // Only one extension
        let path = Path::new("/a/b/c/file_name.tsx");
        assert_eq!(path.multiple_extensions(), None);

        // Stories sample extension
        let path = Path::new("/a/b/c/file_name.stories.tsx");
        assert_eq!(path.multiple_extensions(), Some("stories.tsx".to_string()));

        // Longer sample extension
        let path = Path::new("/a/b/c/long.app.tar.gz");
        assert_eq!(path.multiple_extensions(), Some("app.tar.gz".to_string()));
    }

    #[test]
    fn test_strip_path_suffix() {
        let base = Path::new("/a/b/c/file_name");
        let suffix = Path::new("file_name");
        assert_eq!(strip_path_suffix(base, suffix), Some(Path::new("/a/b/c")));

        let base = Path::new("/a/b/c/file_name.tsx");
        let suffix = Path::new("file_name.tsx");
        assert_eq!(strip_path_suffix(base, suffix), Some(Path::new("/a/b/c")));

        let base = Path::new("/a/b/c/file_name.stories.tsx");
        let suffix = Path::new("c/file_name.stories.tsx");
        assert_eq!(strip_path_suffix(base, suffix), Some(Path::new("/a/b")));

        let base = Path::new("/a/b/c/long.app.tar.gz");
        let suffix = Path::new("b/c/long.app.tar.gz");
        assert_eq!(strip_path_suffix(base, suffix), Some(Path::new("/a")));

        let base = Path::new("/a/b/c/long.app.tar.gz");
        let suffix = Path::new("/a/b/c/long.app.tar.gz");
        assert_eq!(strip_path_suffix(base, suffix), Some(Path::new("")));

        let base = Path::new("/a/b/c/long.app.tar.gz");
        let suffix = Path::new("/a/b/c/no_match.app.tar.gz");
        assert_eq!(strip_path_suffix(base, suffix), None);

        let base = Path::new("/a/b/c/long.app.tar.gz");
        let suffix = Path::new("app.tar.gz");
        assert_eq!(strip_path_suffix(base, suffix), None);
    }

    #[test]
    fn test_strip_prefix() {
        let expected = [
            (
                PathStyle::Posix,
                "/a/b/c",
                "/a/b",
                Some(rel_path("c").into_arc()),
            ),
            (
                PathStyle::Posix,
                "/a/b/c",
                "/a/b/",
                Some(rel_path("c").into_arc()),
            ),
            (
                PathStyle::Posix,
                "/a/b/c",
                "/",
                Some(rel_path("a/b/c").into_arc()),
            ),
            (PathStyle::Posix, "/a/b/c", "", None),
            (PathStyle::Posix, "/a/b//c", "/a/b/", None),
            (PathStyle::Posix, "/a/bc", "/a/b", None),
            (
                PathStyle::Posix,
                "/a/b/c",
                "/a/b/c",
                Some(rel_path("").into_arc()),
            ),
            (
                PathStyle::Windows,
                "C:\\a\\b\\c",
                "C:\\a\\b",
                Some(rel_path("c").into_arc()),
            ),
            (
                PathStyle::Windows,
                "C:\\a\\b\\c",
                "C:\\a\\b\\",
                Some(rel_path("c").into_arc()),
            ),
            (
                PathStyle::Windows,
                "C:\\a\\b\\c",
                "C:\\",
                Some(rel_path("a/b/c").into_arc()),
            ),
            (PathStyle::Windows, "C:\\a\\b\\c", "", None),
            (PathStyle::Windows, "C:\\a\\b\\\\c", "C:\\a\\b\\", None),
            (PathStyle::Windows, "C:\\a\\bc", "C:\\a\\b", None),
            (
                PathStyle::Windows,
                "C:\\a\\b/c",
                "C:\\a\\b",
                Some(rel_path("c").into_arc()),
            ),
            (
                PathStyle::Windows,
                "C:\\a\\b/c",
                "C:\\a\\b\\",
                Some(rel_path("c").into_arc()),
            ),
            (
                PathStyle::Windows,
                "C:\\a\\b/c",
                "C:\\a\\b/",
                Some(rel_path("c").into_arc()),
            ),
        ];
        let actual = expected.clone().map(|(style, child, parent, _)| {
            (
                style,
                child,
                parent,
                style
                    .strip_prefix(child.as_ref(), parent.as_ref())
                    .map(|rel_path| rel_path.into_arc()),
            )
        });
        pretty_assertions::assert_eq!(actual, expected);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_wsl_path() {
        use super::WslPath;
        let path = "/a/b/c";
        assert_eq!(WslPath::from_path(&path), None);

        let path = r"\\wsl.localhost";
        assert_eq!(WslPath::from_path(&path), None);

        let path = r"\\wsl.localhost\Distro";
        assert_eq!(
            WslPath::from_path(&path),
            Some(WslPath {
                distro: "Distro".to_owned(),
                path: "/".into(),
            })
        );

        let path = r"\\wsl.localhost\Distro\blue";
        assert_eq!(
            WslPath::from_path(&path),
            Some(WslPath {
                distro: "Distro".to_owned(),
                path: "/blue".into()
            })
        );

        let path = r"\\wsl$\archlinux\tomato\.\paprika\..\aubergine.txt";
        assert_eq!(
            WslPath::from_path(&path),
            Some(WslPath {
                distro: "archlinux".to_owned(),
                path: "/tomato/paprika/../aubergine.txt".into()
            })
        );

        let path = r"\\windows.localhost\Distro\foo";
        assert_eq!(WslPath::from_path(&path), None);
    }

    #[test]
    fn test_url_to_file_path_ext_posix_basic() {
        use super::UrlExt;

        let url = url::Url::parse("file:///home/user/file.txt").unwrap();
        assert_eq!(
            url.to_file_path_ext(PathStyle::Posix),
            Ok(PathBuf::from("/home/user/file.txt"))
        );

        let url = url::Url::parse("file:///").unwrap();
        assert_eq!(
            url.to_file_path_ext(PathStyle::Posix),
            Ok(PathBuf::from("/"))
        );

        let url = url::Url::parse("file:///a/b/c/d/e").unwrap();
        assert_eq!(
            url.to_file_path_ext(PathStyle::Posix),
            Ok(PathBuf::from("/a/b/c/d/e"))
        );
    }

    #[test]
    fn test_url_to_file_path_ext_posix_percent_encoding() {
        use super::UrlExt;

        let url = url::Url::parse("file:///home/user/file%20with%20spaces.txt").unwrap();
        assert_eq!(
            url.to_file_path_ext(PathStyle::Posix),
            Ok(PathBuf::from("/home/user/file with spaces.txt"))
        );

        let url = url::Url::parse("file:///path%2Fwith%2Fencoded%2Fslashes").unwrap();
        assert_eq!(
            url.to_file_path_ext(PathStyle::Posix),
            Ok(PathBuf::from("/path/with/encoded/slashes"))
        );

        let url = url::Url::parse("file:///special%23chars%3F.txt").unwrap();
        assert_eq!(
            url.to_file_path_ext(PathStyle::Posix),
            Ok(PathBuf::from("/special#chars?.txt"))
        );
    }

    #[test]
    fn test_url_to_file_path_ext_posix_localhost() {
        use super::UrlExt;

        let url = url::Url::parse("file://localhost/home/user/file.txt").unwrap();
        assert_eq!(
            url.to_file_path_ext(PathStyle::Posix),
            Ok(PathBuf::from("/home/user/file.txt"))
        );
    }

    #[test]
    fn test_url_to_file_path_ext_posix_rejects_host() {
        use super::UrlExt;

        let url = url::Url::parse("file://somehost/home/user/file.txt").unwrap();
        assert_eq!(url.to_file_path_ext(PathStyle::Posix), Err(()));
    }

    #[test]
    fn test_url_to_file_path_ext_posix_windows_drive_letter() {
        use super::UrlExt;

        let url = url::Url::parse("file:///C:").unwrap();
        assert_eq!(
            url.to_file_path_ext(PathStyle::Posix),
            Ok(PathBuf::from("/C:/"))
        );

        let url = url::Url::parse("file:///D|").unwrap();
        assert_eq!(
            url.to_file_path_ext(PathStyle::Posix),
            Ok(PathBuf::from("/D|/"))
        );
    }

    #[test]
    fn test_url_to_file_path_ext_windows_basic() {
        use super::UrlExt;

        let url = url::Url::parse("file:///C:/Users/user/file.txt").unwrap();
        assert_eq!(
            url.to_file_path_ext(PathStyle::Windows),
            Ok(PathBuf::from("C:\\Users\\user\\file.txt"))
        );

        let url = url::Url::parse("file:///D:/folder/subfolder/file.rs").unwrap();
        assert_eq!(
            url.to_file_path_ext(PathStyle::Windows),
            Ok(PathBuf::from("D:\\folder\\subfolder\\file.rs"))
        );

        let url = url::Url::parse("file:///C:/").unwrap();
        assert_eq!(
            url.to_file_path_ext(PathStyle::Windows),
            Ok(PathBuf::from("C:\\"))
        );
    }

    #[test]
    fn test_url_to_file_path_ext_windows_encoded_drive_letter() {
        use super::UrlExt;

        let url = url::Url::parse("file:///C%3A/Users/file.txt").unwrap();
        assert_eq!(
            url.to_file_path_ext(PathStyle::Windows),
            Ok(PathBuf::from("C:\\Users\\file.txt"))
        );

        let url = url::Url::parse("file:///c%3a/Users/file.txt").unwrap();
        assert_eq!(
            url.to_file_path_ext(PathStyle::Windows),
            Ok(PathBuf::from("c:\\Users\\file.txt"))
        );

        let url = url::Url::parse("file:///D%3A/folder/file.txt").unwrap();
        assert_eq!(
            url.to_file_path_ext(PathStyle::Windows),
            Ok(PathBuf::from("D:\\folder\\file.txt"))
        );

        let url = url::Url::parse("file:///d%3A/folder/file.txt").unwrap();
        assert_eq!(
            url.to_file_path_ext(PathStyle::Windows),
            Ok(PathBuf::from("d:\\folder\\file.txt"))
        );
    }

    #[test]
    fn test_url_to_file_path_ext_windows_unc_path() {
        use super::UrlExt;

        let url = url::Url::parse("file://server/share/path/file.txt").unwrap();
        assert_eq!(
            url.to_file_path_ext(PathStyle::Windows),
            Ok(PathBuf::from("\\\\server\\share\\path\\file.txt"))
        );

        let url = url::Url::parse("file://server/share").unwrap();
        assert_eq!(
            url.to_file_path_ext(PathStyle::Windows),
            Ok(PathBuf::from("\\\\server\\share"))
        );
    }

    #[test]
    fn test_url_to_file_path_ext_windows_percent_encoding() {
        use super::UrlExt;

        let url = url::Url::parse("file:///C:/Users/user/file%20with%20spaces.txt").unwrap();
        assert_eq!(
            url.to_file_path_ext(PathStyle::Windows),
            Ok(PathBuf::from("C:\\Users\\user\\file with spaces.txt"))
        );

        let url = url::Url::parse("file:///C:/special%23chars%3F.txt").unwrap();
        assert_eq!(
            url.to_file_path_ext(PathStyle::Windows),
            Ok(PathBuf::from("C:\\special#chars?.txt"))
        );
    }

    #[test]
    fn test_url_to_file_path_ext_windows_invalid_drive() {
        use super::UrlExt;

        let url = url::Url::parse("file:///1:/path/file.txt").unwrap();
        assert_eq!(url.to_file_path_ext(PathStyle::Windows), Err(()));

        let url = url::Url::parse("file:///CC:/path/file.txt").unwrap();
        assert_eq!(url.to_file_path_ext(PathStyle::Windows), Err(()));

        let url = url::Url::parse("file:///C/path/file.txt").unwrap();
        assert_eq!(url.to_file_path_ext(PathStyle::Windows), Err(()));

        let url = url::Url::parse("file:///invalid").unwrap();
        assert_eq!(url.to_file_path_ext(PathStyle::Windows), Err(()));
    }

    #[test]
    fn test_url_to_file_path_ext_non_file_scheme() {
        use super::UrlExt;

        let url = url::Url::parse("http://example.com/path").unwrap();
        assert_eq!(url.to_file_path_ext(PathStyle::Posix), Err(()));
        assert_eq!(url.to_file_path_ext(PathStyle::Windows), Err(()));

        let url = url::Url::parse("https://example.com/path").unwrap();
        assert_eq!(url.to_file_path_ext(PathStyle::Posix), Err(()));
        assert_eq!(url.to_file_path_ext(PathStyle::Windows), Err(()));
    }

    #[test]
    fn test_url_to_file_path_ext_windows_localhost() {
        use super::UrlExt;

        let url = url::Url::parse("file://localhost/C:/Users/file.txt").unwrap();
        assert_eq!(
            url.to_file_path_ext(PathStyle::Windows),
            Ok(PathBuf::from("C:\\Users\\file.txt"))
        );
    }
}
