//! Session completion orchestration, independent of a terminal view or window.
use crate::{
    SuggestionSettings,
    completion::Candidate,
    path_suggestions::{Entry, PathQuery},
    project_completions,
    suggestions::Suggestions,
};
use gpui::{BackgroundExecutor, Task};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
const CACHE_TTL: Duration = Duration::from_secs(3);
#[derive(Clone)]
pub(super) enum Backend {
    Local { home: Option<PathBuf> },
    Remote(terminal::SshFs),
}
impl Backend {
    pub fn local() -> Self {
        Self::Local {
            home: std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from),
        }
    }
}
#[derive(Clone)]
struct PathCache {
    directory: String,
    loaded: Instant,
    entries: Vec<Entry>,
}
#[derive(Clone)]
struct ProjectCache {
    provider: project_completions::Provider,
    cwd: String,
    loaded: Instant,
    values: Vec<String>,
}
#[derive(Clone)]
struct Identity {
    generation: u64,
    revision: u64,
    cwd: String,
    query: PathQuery,
}
enum Query {
    Paths {
        cache: Option<PathCache>,
    },
    Project {
        query: project_completions::Query,
        cache: Option<ProjectCache>,
    },
}
pub(super) struct Request {
    identity: Identity,
    backend: Backend,
    query: Query,
    manual: bool,
}
enum CacheUpdate {
    Paths(PathCache),
    Project(ProjectCache),
}
pub(super) struct Response {
    identity: Identity,
    candidates: Vec<Candidate>,
    cache: CacheUpdate,
}
pub(super) struct CompletionController {
    pub model: Suggestions,
    pub task: Task<()>,
    generation: u64,
    paths: Option<PathCache>,
    project: Option<ProjectCache>,
}
impl CompletionController {
    pub fn new(settings: SuggestionSettings) -> Self {
        Self {
            model: Suggestions::new(settings),
            task: Task::ready(()),
            generation: 0,
            paths: None,
            project: None,
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
        self.paths = None;
        self.project = None;
    }
    pub fn request(&self, manual: bool, backend: Backend) -> Option<Request> {
        let query = self.model.path_query()?;
        let project = self
            .model
            .settings
            .dynamic
            .then(|| project_completions::Query::parse(&query))
            .flatten()
            .filter(|query| self.model.has_command(query.command()));
        let query_kind = if let Some(query) = project {
            Query::Project {
                query,
                cache: self.project.clone(),
            }
        } else if self.model.settings.paths {
            Query::Paths {
                cache: self.paths.clone(),
            }
        } else {
            return None;
        };
        Some(Request {
            identity: Identity {
                generation: self.generation,
                revision: self.model.revision,
                cwd: self.model.cwd.clone().unwrap_or_default(),
                query,
            },
            backend,
            query: query_kind,
            manual,
        })
    }
    /// One gate for all asynchronous sources, including identical text on a newer prompt.
    pub fn apply(&mut self, response: Response) -> bool {
        let identity = &response.identity;
        if identity.generation != self.generation
            || identity.revision != self.model.revision
            || self.model.cwd.as_deref().unwrap_or("") != identity.cwd
            || self.model.path_query().as_ref() != Some(&identity.query)
        {
            return false;
        }
        match response.cache {
            CacheUpdate::Paths(cache) => self.paths = Some(cache),
            CacheUpdate::Project(cache) => self.project = Some(cache),
        }
        self.model.add_paths(response.candidates);
        true
    }
}
impl Request {
    pub async fn resolve(self, executor: BackgroundExecutor) -> Option<Response> {
        let identity = self.identity;
        let cwd = &identity.cwd;
        let path = &identity.query;
        let (candidates, cache) = match self.query {
            Query::Project { query, cache } => {
                if cwd.is_empty() {
                    return None;
                }
                let values = if let Some(cache) = cache.filter(|cache| {
                    !self.manual
                        && cache.provider == query.provider
                        && cache.cwd == *cwd
                        && cache.loaded.elapsed() < CACHE_TTL
                }) {
                    cache.values
                } else {
                    match &self.backend {
                        Backend::Remote(fs) => {
                            let request =
                                Box::pin(project_completions::remote(query.provider, cwd, fs));
                            let deadline = Box::pin(executor.timer(Duration::from_secs(5)));
                            match futures::future::select(request, deadline).await {
                                futures::future::Either::Left((values, _)) => values,
                                futures::future::Either::Right(_) => return None,
                            }
                        }
                        Backend::Local { .. } => {
                            let provider = query.provider;
                            let cwd = PathBuf::from(cwd);
                            executor
                                .spawn(async move { project_completions::local(provider, &cwd) })
                                .await
                        }
                    }
                };
                let candidates = query.candidates(values.clone());
                (
                    candidates,
                    CacheUpdate::Project(ProjectCache {
                        provider: query.provider,
                        cwd: cwd.clone(),
                        loaded: Instant::now(),
                        values,
                    }),
                )
            }
            Query::Paths { cache } => {
                let directory = match &self.backend {
                    Backend::Remote(fs) => {
                        let home = if path.expand_home {
                            fs.home_dir().await.ok()
                        } else {
                            None
                        };
                        path.remote_directory(cwd, home.as_deref())?
                    }
                    Backend::Local { home } => {
                        let directory = if path.expand_home {
                            home.as_ref()?.join(&path.directory[2..])
                        } else if std::path::Path::new(&path.directory).is_absolute() {
                            PathBuf::from(&path.directory)
                        } else {
                            if cwd.is_empty() {
                                return None;
                            }
                            PathBuf::from(cwd).join(&path.directory)
                        };
                        directory.to_string_lossy().into_owned()
                    }
                };
                let entries = if let Some(cache) = cache.filter(|cache| {
                    !self.manual
                        && cache.directory == directory
                        && cache.loaded.elapsed() < CACHE_TTL
                }) {
                    cache.entries
                } else {
                    match self.backend {
                        Backend::Remote(fs) => fs
                            .list_completion_dir(directory.clone())
                            .await
                            .ok()?
                            .into_iter()
                            .take(4096)
                            .map(|entry| Entry {
                                name: entry.name,
                                directory: entry.is_dir,
                            })
                            .collect(),
                        Backend::Local { .. } => {
                            let directory = directory.clone();
                            executor
                                .spawn(async move {
                                    std::fs::read_dir(directory)
                                        .ok()
                                        .into_iter()
                                        .flatten()
                                        .take(4096)
                                        .filter_map(|entry| {
                                            let entry = entry.ok()?;
                                            Some(Entry {
                                                name: entry.file_name().into_string().ok()?,
                                                directory: entry.path().is_dir(),
                                            })
                                        })
                                        .collect::<Vec<_>>()
                                })
                                .await
                        }
                    }
                };
                let candidates = path.candidates(entries.clone());
                (
                    candidates,
                    CacheUpdate::Paths(PathCache {
                        directory,
                        entries,
                        loaded: Instant::now(),
                    }),
                )
            }
        };
        Some(Response {
            identity,
            candidates,
            cache,
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
        let entries = vec![Entry {
            name: "sample.txt".into(),
            directory: false,
        }];
        Response {
            candidates: request.identity.query.candidates(entries.clone()),
            identity: request.identity,
            cache: CacheUpdate::Paths(PathCache {
                directory: "/project/".into(),
                loaded: Instant::now(),
                entries,
            }),
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
        assert!(controller.paths.is_none() && controller.project.is_none());
    }
}
