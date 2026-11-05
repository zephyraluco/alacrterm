//! Text decoding and save revision state, independent of the window.
use crate::format;
use std::path::Path;
pub(super) const MAX_TEXT_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RemoteVersion {
    pub(super) size: u64,
    digest: [u8; 32],
}
impl RemoteVersion {
    pub(super) fn of(bytes: &[u8]) -> Self {
        use sha2::Digest as _;
        Self {
            size: bytes.len() as u64,
            digest: sha2::Sha256::digest(bytes).into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineEnding {
    Lf,
    CrLf,
    Cr,
}
impl LineEnding {
    fn text(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
            Self::Cr => "\r",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct TextFormat {
    pub(super) bom: bool,
    endings: Vec<LineEnding>,
    preferred: LineEnding,
}
impl TextFormat {
    pub(super) fn decode(source: &str) -> (String, Self) {
        let bom = source.starts_with('\u{feff}');
        let source = source.strip_prefix('\u{feff}').unwrap_or(source);
        let mut text = String::with_capacity(source.len());
        let mut endings = Vec::new();
        let mut chars = source.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                '\r' => {
                    let ending = if chars.peek() == Some(&'\n') {
                        chars.next();
                        LineEnding::CrLf
                    } else {
                        LineEnding::Cr
                    };
                    endings.push(ending);
                    text.push('\n');
                }
                '\n' => {
                    endings.push(LineEnding::Lf);
                    text.push('\n');
                }
                _ => text.push(ch),
            }
        }
        let preferred = endings.first().copied().unwrap_or(LineEnding::Lf);
        (
            text,
            Self {
                bom,
                endings,
                preferred,
            },
        )
    }
    pub(super) fn encoding(&self) -> &'static str {
        if self.bom { "UTF-8 with BOM" } else { "UTF-8" }
    }
    pub(super) fn line_endings(&self) -> &'static str {
        if self.endings.iter().any(|ending| *ending != self.preferred) {
            return "Mixed";
        }
        match self.preferred {
            LineEnding::Lf => "LF",
            LineEnding::CrLf => "CRLF",
            LineEnding::Cr => "CR",
        }
    }
    pub(super) fn encode(&self, text: &str) -> Vec<u8> {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let text = normalized.as_str();
        let mut bytes = Vec::with_capacity(text.len());
        if self.bom {
            bytes.extend_from_slice(&[0xef, 0xbb, 0xbf]);
        }
        for (index, line) in text.split_inclusive('\n').enumerate() {
            if let Some(line) = line.strip_suffix('\n') {
                bytes.extend_from_slice(line.as_bytes());
                bytes.extend_from_slice(
                    self.endings
                        .get(index)
                        .copied()
                        .unwrap_or(self.preferred)
                        .text()
                        .as_bytes(),
                );
            } else {
                bytes.extend_from_slice(line.as_bytes());
            }
        }
        bytes
    }
}

pub(super) struct Loaded {
    pub(super) text: String,
    pub(super) format: TextFormat,
    pub(super) version: RemoteVersion,
}

pub(super) struct TextDocument {
    pub(super) format: TextFormat,
    pub(super) version: RemoteVersion,
    pub(super) revisions: Revisions,
    pub(super) saving: Option<Saving>,
    pub(super) outcome: Option<Result<(), String>>,
}
impl TextDocument {
    pub(super) fn new(format: TextFormat, version: RemoteVersion) -> Self {
        Self {
            format,
            version,
            revisions: Revisions::default(),
            saving: None,
            outcome: None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SavePhase {
    Checking,
    Uploading,
}

/// 「改动到哪一版 / 写回到哪一版」的账本。
///
/// 分成两个数是因为**写回是异步的**:开写时抓的是某一版,路上用户还能接着改 ——
/// 回来后不能把「已写回」记到**最新**那一版上,否则那句「已写回」会盖住还没写回的改动
/// (见 [`Revisions::mark_saved`] 的调用处)。
#[derive(Default)]
pub(super) struct Revisions {
    /// 编辑器每改一次 +1。
    pub(super) current: u64,
    /// 最近一次**成功**写回远端的版本。
    pub(super) saved: u64,
}

impl Revisions {
    /// 编辑器改动了一笔。
    pub(super) fn edited(&mut self) {
        self.current += 1;
    }

    /// 记下「**这一版**已经写回远端」。⚠️ 传的必须是开写时那份快照的版本号,不是 `current`。
    pub(super) fn mark_saved(&mut self, revision: u64) {
        self.saved = revision;
    }

    pub(super) fn is_dirty(&self) -> bool {
        self.current != self.saved
    }
}

/// 一次在跑的写回。
pub(super) struct Saving {
    pub(super) version: RemoteVersion,
    pub(super) phase: SavePhase,
    pub(super) id: SaveOperationId,
    /// 抓那份文本时的版本号(见 [`Revisions`])。
    pub(super) revision: u64,
    /// 已经写过去多少字节。
    pub(super) done: u64,
    /// 这次要写多少字节(工具条上那行进度小字的分母)。
    pub(super) total: u64,
}

impl Saving {
    pub(super) fn matches(saving: Option<&Self>, operation_id: SaveOperationId) -> bool {
        saving.is_some_and(|saving| saving.id == operation_id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SaveOperationId(pub(super) u64);

impl SaveOperationId {
    pub(super) fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        Self(
            NEXT.try_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |value| value.checked_add(1),
            )
            .expect("save operation identity exhausted"),
        )
    }
}

/// 读文本:先按大小把大的挡掉,再要求是 UTF-8。
pub(super) fn read_text(path: &Path) -> Result<Loaded, String> {
    let metadata = std::fs::metadata(path).map_err(|error| format!("本地文件出错:{error}"))?;
    if metadata.len() > MAX_TEXT_BYTES {
        return Err(format!(
            "文件有 {},超过文本编辑的 {} 上限,就不在应用里打开了。",
            format::size(metadata.len()),
            format::size(MAX_TEXT_BYTES)
        ));
    }
    let bytes = std::fs::read(path).map_err(|error| format!("本地文件出错:{error}"))?;
    if bytes.contains(&0) {
        return Err("文件包含二进制内容，请使用系统程序打开。".into());
    }
    let version = RemoteVersion::of(&bytes);
    let source = String::from_utf8(bytes)
        .map_err(|_| "这不是 UTF-8 文本，请使用系统程序打开。".to_string())?;
    let (text, format) = TextFormat::decode(&source);
    Ok(Loaded {
        text,
        format,
        version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decoding_and_encoding_round_trip_bom_unicode_and_line_endings() {
        for source in [
            "plain\ntext\n",
            "windows\r\ntext\r\n",
            "old\rtext\r",
            "mixed\r\nline\nlast\r",
            "\u{feff}中文\r\nnext\r\n",
            "no newline",
        ] {
            let (text, format) = TextFormat::decode(source);
            assert!(!text.contains('\r'));
            assert!(!text.starts_with('\u{feff}'));
            assert_eq!(format.encode(&text), source.as_bytes());
        }
    }
    #[test]
    fn new_lines_follow_the_document_style_without_doubled_carriage_returns() {
        let (_, format) = TextFormat::decode("first\r\n");
        assert_eq!(format.encode("first\nadded\n"), b"first\r\nadded\r\n");
        assert_eq!(format.encode("first\r\nadded\r\n"), b"first\r\nadded\r\n");
    }
    #[test]
    fn same_size_content_changes_have_different_remote_versions() {
        assert_ne!(RemoteVersion::of(b"before"), RemoteVersion::of(b"after!"));
        assert_eq!(RemoteVersion::of(b"same"), RemoteVersion::of(b"same"));
    }
    #[test]
    fn saving_a_snapshot_never_marks_newer_edits_as_saved() {
        let mut revisions = Revisions::default();
        revisions.edited();
        let snapshot = revisions.current;
        revisions.edited();
        revisions.mark_saved(snapshot);
        assert!(revisions.is_dirty());
    }
}
