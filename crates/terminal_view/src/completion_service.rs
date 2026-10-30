//! Host-provided candidates. This contract performs no filesystem or project work.
use super::{SuggestionSettings, completion::Candidate};
use futures::future::BoxFuture;
use gpui::BackgroundExecutor;

pub struct Context<'a> {
    pub text: &'a str,
    pub cursor: usize,
    pub powershell: Option<bool>,
    pub cwd: &'a str,
    pub history: &'a [String],
    pub commands: &'a [String],
    pub settings: &'a SuggestionSettings,
}

#[derive(Clone)]
pub enum Backend {
    Local,
    Remote(terminal::SshFs),
}

pub trait CompletionJob: Send {
    fn resolve(self: Box<Self>, executor: BackgroundExecutor)
    -> BoxFuture<'static, Vec<Candidate>>;
}

pub trait CompletionService: Send + Sync {
    fn immediate(&self, context: &Context<'_>) -> Vec<Candidate>;
    fn deferred(
        &self,
        context: &Context<'_>,
        backend: Backend,
        manual: bool,
    ) -> Option<Box<dyn CompletionJob>>;
    fn filter_text(&self, context: &Context<'_>, candidate: &Candidate) -> String {
        context
            .text
            .get(candidate.replacement.start..context.cursor)
            .unwrap_or("")
            .into()
    }
    /// Reuse cached results only when the same replacement context still applies.
    fn reuse_candidate(
        &self,
        previous: &Context<'_>,
        current: &Context<'_>,
        candidate: &Candidate,
    ) -> Option<Candidate> {
        let item = rebase_candidate(previous, current, candidate)?;
        let query = current.text.get(item.replacement.start..current.cursor)?;
        let matches = if current.powershell == Some(true) {
            item.insert_text
                .to_lowercase()
                .starts_with(&query.to_lowercase())
        } else {
            item.insert_text.starts_with(query)
        };
        matches.then_some(item)
    }
    fn reset(&self) {}
}

/// A candidate can only move across an edit inside its original replacement range.
pub fn rebase_candidate(
    previous: &Context<'_>,
    current: &Context<'_>,
    candidate: &Candidate,
) -> Option<Candidate> {
    if previous.cwd != current.cwd || previous.powershell != current.powershell {
        return None;
    }
    let range = &candidate.replacement;
    if !(range.start..=range.end).contains(&previous.cursor) {
        return None;
    }
    let prefix = previous.text.get(..range.start)?;
    let suffix = previous.text.get(range.end..)?;
    if !current.text.starts_with(prefix) || !current.text.ends_with(suffix) {
        return None;
    }
    let end = current.text.len().checked_sub(suffix.len())?;
    if range.start > end
        || !(range.start..=end).contains(&current.cursor)
        || !current.text.is_char_boundary(end)
    {
        return None;
    }
    let mut item = candidate.clone();
    item.replacement = range.start..end;
    Some(item)
}
