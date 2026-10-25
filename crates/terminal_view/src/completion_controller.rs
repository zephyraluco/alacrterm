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
pub(super) struct CompletionController {
    pub model: Suggestions,
    pub task: Task<()>,
    generation: u64,
    cache: Vec<CacheEntry>,
}
impl CompletionController {
    pub fn new(settings: SuggestionSettings) -> Self {
        Self {
            model: Suggestions::new(settings),
            task: Task::ready(()),
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
        self.model.active
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
