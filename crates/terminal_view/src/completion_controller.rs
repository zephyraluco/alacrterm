//! Session orchestration: query selection, cancellation and result identity.
use crate::{
    SuggestionSettings,
    completion_provider::{Provider, Work},
    completion_resources::{self, Backend, CacheEntry},
    path_suggestions::PathQuery,
    suggestions::Suggestions,
};
use gpui::{BackgroundExecutor, Task};
#[derive(Clone)]
struct Identity {
    generation: u64,
    revision: u64,
    cwd: String,
    query: PathQuery,
}
pub(super) struct Request {
    identity: Identity,
    work: completion_resources::Request,
}
pub(super) struct Response {
    identity: Identity,
    data: completion_resources::Response,
}
pub(super) enum Acceptance {
    None,
    Refresh,
    Insert { bytes: String, retrigger: bool },
}
pub(super) struct CompletionController {
    pub model: Suggestions,
    pub task: Task<()>,
    tracking: crate::prompt_input::InputTracking,
    generation: u64,
    cache: Vec<CacheEntry>,
}
impl CompletionController {
    pub fn new(settings: SuggestionSettings) -> Self {
        Self {
            model: Suggestions::new(settings),
            task: Task::ready(()),
            tracking: Default::default(),
            generation: 0,
            cache: Vec::new(),
        }
    }
    pub fn cancel(&mut self) {
        self.task = Task::ready(());
        self.generation = self.generation.wrapping_add(1);
    }
    pub fn begin(&mut self, manual: bool) -> bool {
        self.cancel();
        if !manual {
            self.model.candidates.clear();
        }
        self.model.is_editing()
    }
    pub fn configure(&mut self, settings: SuggestionSettings) {
        self.cancel();
        self.model.settings = settings;
        self.model.dismiss();
        if !settings.history {
            self.model.clear_history();
        }
    }
    pub fn reset_session(&mut self) {
        self.cancel();
        self.model.invalidate();
        self.model.add_command("");
        self.model.cwd = None;
        self.model.powershell = None;
        self.cache.clear();
        self.tracking.reset();
    }
    pub fn has_input_anchor(&self) -> bool {
        self.tracking.has_anchor()
    }
    pub fn capture_input(&mut self, content: &terminal::Content) {
        if self.model.is_editing() && self.model.text.is_empty() {
            self.tracking.capture(content);
        }
    }
    pub fn on_prompt(&mut self, event: terminal::PromptEvent, generation: u64) -> bool {
        if event != terminal::PromptEvent::Input {
            self.cancel();
            self.tracking.reset();
        }
        if event == terminal::PromptEvent::Input
            && self.tracking.has_anchor()
            && !self.model.text.is_empty()
        {
            self.request_recovery(generation)
        } else {
            self.model.phase(event);
            false
        }
    }
    pub fn request_recovery(&mut self, generation: u64) -> bool {
        if !self.tracking.request(generation, std::time::Instant::now()) {
            return false;
        }
        self.cancel();
        self.model.invalidate();
        true
    }
    pub fn recovery_due(&self) -> bool {
        self.tracking.due(std::time::Instant::now())
    }
    pub fn recover_input(&mut self, content: &terminal::Content, generation: u64) -> bool {
        let Some((text, cursor)) =
            self.tracking
                .recover(content, generation, std::time::Instant::now())
        else {
            return false;
        };
        self.model.synchronize(text, cursor);
        true
    }
    pub fn submit(&mut self) {
        self.cancel();
        self.model.submit();
        self.tracking.reset();
    }
    pub fn invalidate_input(&mut self) {
        self.cancel();
        self.model.invalidate();
        self.tracking.reset();
    }
    pub fn dismiss(&mut self) {
        self.cancel();
        self.model.dismiss();
    }
    pub fn accept(&mut self, content: Option<&terminal::Content>) -> Acceptance {
        if let Some((text, cursor)) = content.and_then(|content| self.tracking.current(content)) {
            if text != self.model.text || cursor != self.model.cursor {
                self.model.synchronize(text, cursor);
                return Acceptance::Refresh;
            }
        }
        let retrigger = self
            .model
            .candidates
            .get(self.model.selected)
            .is_some_and(|item| item.retrigger);
        let Some(bytes) = self.model.accept() else {
            return Acceptance::None;
        };
        if retrigger {
            self.model.trigger();
        }
        Acceptance::Insert { bytes, retrigger }
    }
    pub fn request(&self, manual: bool, backend: Backend) -> Option<Request> {
        let context = self.model.provider_context();
        let path = context.path.clone()?;
        // Project arguments retain priority over generic file suggestions.
        let lookup = [Provider::Project, Provider::Paths]
            .into_iter()
            .find_map(|provider| match provider.query(&context) {
                Some(Work::Lookup(query)) => Some(query),
                _ => None,
            })?;
        let cwd = self.model.cwd.clone().unwrap_or_default();
        Some(Request {
            identity: Identity {
                generation: self.generation,
                revision: self.model.revision,
                cwd: cwd.clone(),
                query: path,
            },
            work: completion_resources::Request {
                lookup,
                backend,
                cache: self.cache.clone(),
                manual,
                cwd,
            },
        })
    }
    /// One gate for every deferred source, including identical text on a newer prompt.
    pub fn apply(&mut self, response: Response) -> bool {
        let identity = &response.identity;
        if identity.generation != self.generation
            || identity.revision != self.model.revision
            || self.model.cwd.as_deref().unwrap_or("") != identity.cwd
            || self.model.path_query().as_ref() != Some(&identity.query)
        {
            return false;
        }
        // Keep one path cache and one project cache, matching the original bound.
        self.cache
            .retain(|entry| !entry.key.same_source(&response.data.cache.key));
        self.cache.push(response.data.cache);
        self.model.merge_candidates(response.data.candidates);
        true
    }
}
impl Request {
    pub async fn resolve(self, executor: BackgroundExecutor) -> Option<Response> {
        Some(Response {
            identity: self.identity,
            data: self.work.resolve(executor).await?,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use terminal::PromptEvent;
    fn controller() -> CompletionController {
        let mut controller = CompletionController::new(Default::default());
        controller.model.phase(PromptEvent::Prompt);
        controller.model.phase(PromptEvent::Input);
        controller.model.context("shell", "posix");
        controller.model.context("cwd", "/project");
        controller.model.input("cat sa");
        controller.begin(false);
        controller.model.refresh(false);
        controller
    }
    fn result(request: Request) -> Response {
        let entries = vec![crate::path_suggestions::Entry {
            name: "sample.txt".into(),
            directory: false,
        }];
        Response {
            data: completion_resources::Response {
                candidates: request.identity.query.candidates(entries.clone()),
                cache: CacheEntry {
                    key: completion_resources::CacheKey::Directory("/project/".into()),
                    loaded: std::time::Instant::now(),
                    data: completion_resources::Data::Paths(entries),
                },
            },
            identity: request.identity,
        }
    }

    #[test]
    fn submitting_or_finishing_a_prompt_discards_pending_native_recovery() {
        let mut controller = CompletionController::new(Default::default());
        controller.on_prompt(PromptEvent::Prompt, 1);
        controller.on_prompt(PromptEvent::Input, 1);
        controller.capture_input(&terminal::Content::default());
        assert!(controller.has_input_anchor());
        controller.model.input("echo hi");
        assert!(controller.request_recovery(2));
        controller.submit();
        assert!(!controller.has_input_anchor());
        assert!(!controller.request_recovery(3));
        controller.on_prompt(PromptEvent::Execute, 3);
        controller.model.input("password-value");
        assert!(controller.model.text.is_empty());
        controller.on_prompt(PromptEvent::Prompt, 4);
        controller.on_prompt(PromptEvent::Input, 4);
        controller.model.trigger();
        assert!(controller.model.candidates.is_empty());
        controller.capture_input(&terminal::Content::default());
        assert!(controller.request_recovery(5));
        controller.on_prompt(PromptEvent::Finished, 6);
        assert!(!controller.has_input_anchor() && !controller.recovery_due());
    }
    #[test]
    fn a_new_request_rejects_old_results_even_when_input_is_identical() {
        let mut controller = controller();
        let old = controller.request(false, Backend::local()).unwrap();
        controller.begin(true);
        assert!(!controller.apply(result(old)));
        let current = controller.request(true, Backend::local()).unwrap();
        assert!(controller.apply(result(current)));
        assert_eq!(controller.model.candidates[0].label, "sample.txt");
    }
    #[test]
    fn changed_directory_and_reconnect_reject_inflight_results() {
        let mut controller = controller();
        let old = controller.request(false, Backend::local()).unwrap();
        controller.model.context("cwd", "/another-project");
        assert!(!controller.apply(result(old)));
        let old = controller.request(false, Backend::local()).unwrap();
        controller.reset_session();
        assert!(!controller.apply(result(old)));
        assert!(controller.cache.is_empty());
    }
}
