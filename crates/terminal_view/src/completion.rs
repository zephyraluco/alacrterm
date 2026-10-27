//! Provider-independent completion items. Display text never doubles as shell input.
use std::ops::Range;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    History,
    Command,
    File,
    Directory,
    Subcommand,
    Option,
    Argument,
}
impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::History => "历史",
            Self::Command => "命令",
            Self::File => "文件",
            Self::Directory => "目录",
            Self::Subcommand => "子命令",
            Self::Option => "选项",
            Self::Argument => "参数",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    History,
    Shell,
    Specification,
    Filesystem,
    Project,
}
impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Self::History => "当前会话历史",
            Self::Shell => "当前 shell",
            Self::Specification => "内置命令规格",
            Self::Filesystem => "当前会话文件系统",
            Self::Project => "当前项目元数据",
        }
    }
}
#[derive(Clone, Debug)]
pub struct Candidate {
    pub label: String,
    pub insert_text: String,
    pub replacement: Range<usize>,
    pub kind: Kind,
    pub source: Source,
    pub description: Option<String>,
    pub score: i64,
    pub retrigger: bool,
    pub cursor_back: usize,
}
impl Candidate {
    pub fn new(text: String, input_len: usize, kind: Kind, source: Source, score: i64) -> Self {
        Self {
            label: text.clone(),
            insert_text: text,
            replacement: 0..input_len,
            kind,
            source,
            description: None,
            score,
            retrigger: kind == Kind::Directory,
            cursor_back: 0,
        }
    }
}
/// All sources share one ranking and deduplication pass; reserve no fixed quota.
pub fn rank(items: &mut Vec<Candidate>) {
    items.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.label.cmp(&b.label)));
    let mut seen = std::collections::HashSet::new();
    items.retain(|item| seen.insert((item.replacement.clone(), item.insert_text.clone())));
    items.truncate(4096);
}
