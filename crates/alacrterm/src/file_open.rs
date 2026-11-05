//! Post-download routing: only text enters the editor; all other files use the OS.
use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

use gpui::{App, Context};
use gpui_kit::component::notification::Notification;

use crate::AppRoot;
use gpui::{SharedString, WeakEntity};
use std::path::PathBuf;
use terminal::SshFs;

/// Downloaded file identity and cache lease shared by opening routes.
#[derive(Clone)]
pub(crate) struct FileOpenRequest {
    /// 显示名(远端文件名)。
    pub(crate) name: SharedString,
    /// 远端主机名 —— 状态栏左侧那条「主机:路径」用(见 [`EditorWindow::render_status_bar`])。
    pub(crate) host: String,
    /// 远端路径(文本保存时写回的就是它)。
    pub(crate) remote_path: String,
    /// 这条预览所属的连接实例 —— 不同连接的同名路径是两个标签。
    pub(crate) session: terminal::SessionId,
    /// 已经下到本地的缓存文件。
    pub(crate) local_path: PathBuf,
    /// 缓存文件的租约：标签活着就一直持有，关闭时自动删（见 [`crate::preview_cache`]）。
    pub(crate) _lease: crate::preview_cache::CacheLease,
    /// 字节数(远端报的;0 = 不知道)。
    pub(crate) size: u64,
    /// 远端文件系统句柄 —— 文本保存写回远端用(别的类型用不上)。
    pub(crate) fs: SshFs,
    /// 保存成功后让文件管理器把所在目录重读一次(见 [`crate::editor_window`])。
    pub(crate) app_root: WeakEntity<AppRoot>,
}

const SAMPLE_BYTES: usize = 8192;

fn text_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let extension = Path::new(&lower)
        .extension()
        .and_then(|value| value.to_str());
    extension.is_some_and(|extension| TEXT_EXTENSIONS.contains(&extension))
        || lower.starts_with(".env.")
        || matches!(
            lower.as_str(),
            "makefile"
                | "dockerfile"
                | "license"
                | "readme"
                | "changelog"
                | "gitignore"
                | ".gitignore"
                | ".gitattributes"
                | ".editorconfig"
                | ".bashrc"
                | ".zshrc"
                | ".profile"
        )
}

fn should_edit(path: &Path, name: &str) -> io::Result<bool> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take((SAMPLE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    let truncated = bytes.len() > SAMPLE_BYTES;
    bytes.truncate(SAMPLE_BYTES);
    // Extension hints never turn binary content into an editable document.
    if bytes.contains(&0) || bytes.starts_with(b"%PDF-") {
        return Ok(false);
    }
    if text_name(name) {
        // The editor handles size/encoding errors and offers an external-open action.
        return Ok(true);
    }
    // Unknown extensions belong to their registered system handler. Extensionless
    // files can be scripts or notes, so use a bounded content probe for those.
    if Path::new(name).extension().is_some() {
        return Ok(false);
    }
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(error) if truncated && error.error_len().is_none() => {
            std::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap_or("")
        }
        Err(_) => return Ok(false),
    };
    Ok(!text
        .chars()
        .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t' | '\u{c}')))
}

impl AppRoot {
    pub(crate) fn open_downloaded_file(
        &mut self,
        request: FileOpenRequest,
        cx: &mut Context<Self>,
    ) {
        if self.closing {
            return;
        }
        cx.spawn(async move |this, cx| {
            let path = request.local_path.clone();
            let name = request.name.clone();
            let result = cx
                .background_executor()
                .spawn(async move { should_edit(&path, &name) })
                .await;
            let _ = this.update(cx, |root, cx| {
                if root.closing {
                    return;
                }
                match result {
                    Ok(true) => root.open_editor_window(request, cx),
                    Ok(false) => open_in_system(request, cx),
                    Err(error) => {
                        root.notify(Notification::error(format!("无法打开文件：{error}")), cx)
                    }
                }
            });
        })
        .detach();
    }
}

pub(crate) fn open_in_system(request: FileOpenRequest, cx: &mut App) {
    let path = request.local_path.clone();
    let opening = cx
        .background_executor()
        .spawn(async move { open::that(path) });
    cx.spawn(async move |cx| match opening.await {
        Ok(()) => request._lease.detach(),
        Err(error) => {
            let _ = request.app_root.update(cx, |root, cx| {
                root.notify(
                    Notification::error(format!("系统程序无法打开 {}：{error}", request.name)),
                    cx,
                );
            });
        }
    })
    .detach();
}
const TEXT_EXTENSIONS: [&str; 62] = [
    "txt",
    "text",
    "md",
    "markdown",
    "rst",
    "log",
    "csv",
    "tsv",
    "json",
    "jsonc",
    "json5",
    "toml",
    "yaml",
    "yml",
    "ini",
    "cfg",
    "conf",
    "properties",
    "env",
    "xml",
    "html",
    "htm",
    "css",
    "scss",
    "less",
    "js",
    "mjs",
    "cjs",
    "ts",
    "tsx",
    "jsx",
    "vue",
    "svelte",
    "rs",
    "py",
    "pyi",
    "rb",
    "go",
    "c",
    "h",
    "cc",
    "cpp",
    "cxx",
    "hpp",
    "hxx",
    "java",
    "kt",
    "kts",
    "swift",
    "php",
    "pl",
    "lua",
    "sh",
    "bash",
    "zsh",
    "fish",
    "ps1",
    "bat",
    "cmd",
    "sql",
    "diff",
    "patch",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn classify(name: &str, bytes: &[u8]) -> bool {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("download");
        std::fs::write(&path, bytes).unwrap();
        should_edit(&path, name).unwrap()
    }

    #[test]
    fn source_configuration_and_dotfiles_use_the_editor() {
        for name in [
            "main.rs",
            "clip.ts",
            "README",
            "Makefile",
            "Dockerfile",
            ".env",
            ".env.production",
            ".bashrc",
            ".gitignore",
            "CONFIG.YAML",
            "notes.txt",
        ] {
            assert!(classify(name, b"plain text\n"), "{name}");
        }
    }
    #[test]
    fn non_text_extensions_share_the_system_route() {
        for name in [
            "photo.png",
            "logo.svg",
            "song.mp3",
            "clip.mp4",
            "report.pdf",
            "archive.zip",
            "app.exe",
            "data.bin",
            "unknown.xyz",
        ] {
            assert!(!classify(name, b"opaque content"), "{name}");
        }
    }
    #[test]
    fn binary_content_does_not_enter_the_editor_under_a_text_name() {
        assert!(!classify("fake.txt", b"binary\0payload"));
        assert!(!classify("fake.json", b"%PDF-1.7\n"));
    }
    #[test]
    fn extensionless_files_are_probed_without_losing_unicode() {
        assert!(classify("notes", "中文 notes\n".as_bytes()));
        assert!(classify("empty", b""));
        assert!(!classify("binary", &[0xff, 0xfe, 0x12]));
        assert!(!classify("binary", b"text\x01payload"));
        assert!(!classify("binary", b"%PDF-1.7\n"));
    }
    #[test]
    fn a_utf8_character_crossing_the_probe_boundary_is_not_binary() {
        let mut content = vec![b'a'; SAMPLE_BYTES - 1];
        content.extend_from_slice("中文".as_bytes());
        assert!(classify("notes", &content));
        assert!(!classify("notes", &[0xe4, 0xb8]));
    }
    #[test]
    fn missing_download_is_reported_instead_of_opening_an_empty_editor() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            should_edit(&directory.path().join("missing"), "file.txt")
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
    }
}
