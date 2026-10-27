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
    fn reset(&self) {}
}
