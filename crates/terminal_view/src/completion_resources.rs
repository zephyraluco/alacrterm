//! Filesystem execution, bounded caches and deadlines for deferred providers.
use crate::{
    completion::Candidate, completion_provider::Lookup, path_suggestions::Entry,
    project_completions,
};
use gpui::BackgroundExecutor;
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CacheKey {
    Directory(String),
    Project {
        provider: project_completions::Provider,
        cwd: String,
    },
}
impl CacheKey {
    pub fn same_source(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::Directory(_), Self::Directory(_)) | (Self::Project { .. }, Self::Project { .. })
        )
    }
}
#[derive(Clone)]
pub(super) enum Data {
    Paths(Vec<Entry>),
    Project(Vec<String>),
}
#[derive(Clone)]
pub(super) struct CacheEntry {
    pub key: CacheKey,
    pub loaded: Instant,
    pub data: Data,
}
impl CacheEntry {
    fn valid(&self, key: &CacheKey, manual: bool) -> bool {
        !manual && self.key == *key && self.loaded.elapsed() < CACHE_TTL
    }
}
pub(super) struct Request {
    pub lookup: Lookup,
    pub backend: Backend,
    pub cache: Vec<CacheEntry>,
    pub manual: bool,
    pub cwd: String,
}
pub(super) struct Response {
    pub candidates: Vec<Candidate>,
    pub cache: CacheEntry,
}
impl Request {
    pub async fn resolve(self, executor: BackgroundExecutor) -> Option<Response> {
        // Preserve the remote-project deadline and SFTP's existing per-operation
        // deadlines for paths; the orchestration layer has one policy entry point.
        let deadline = matches!(
            (&self.lookup, &self.backend),
            (Lookup::Project(_), Backend::Remote(_))
        )
        .then_some(Duration::from_secs(5));
        let work = Box::pin(self.resolve_inner(executor.clone()));
        if let Some(deadline) = deadline {
            match futures::future::select(work, Box::pin(executor.timer(deadline))).await {
                futures::future::Either::Left((response, _)) => response,
                futures::future::Either::Right(_) => None,
            }
        } else {
            work.await
        }
    }
    async fn resolve_inner(self, executor: BackgroundExecutor) -> Option<Response> {
        let key = match &self.lookup {
            Lookup::Project(query) => {
                if self.cwd.is_empty() {
                    return None;
                }
                CacheKey::Project {
                    provider: query.provider,
                    cwd: self.cwd.clone(),
                }
            }
            Lookup::Paths(path) => {
                let directory = match &self.backend {
                    Backend::Remote(fs) => {
                        let home = if path.expand_home {
                            fs.home_dir().await.ok()
                        } else {
                            None
                        };
                        path.remote_directory(&self.cwd, home.as_deref())?
                    }
                    Backend::Local { home } => {
                        let path = if path.expand_home {
                            home.as_ref()?.join(&path.directory[2..])
                        } else if std::path::Path::new(&path.directory).is_absolute() {
                            PathBuf::from(&path.directory)
                        } else {
                            if self.cwd.is_empty() {
                                return None;
                            }
                            PathBuf::from(&self.cwd).join(&path.directory)
                        };
                        path.to_string_lossy().into_owned()
                    }
                };
                CacheKey::Directory(directory)
            }
        };
        let data = if let Some(cache) = self
            .cache
            .into_iter()
            .find(|cache| cache.valid(&key, self.manual))
        {
            cache.data
        } else {
            load(&self.lookup, &self.backend, &key, &executor).await?
        };
        let candidates = match (&self.lookup, &data) {
            (Lookup::Paths(path), Data::Paths(entries)) => path.candidates(entries.clone()),
            (Lookup::Project(query), Data::Project(values)) => query.candidates(values.clone()),
            _ => return None,
        };
        Some(Response {
            candidates,
            cache: CacheEntry {
                key,
                loaded: Instant::now(),
                data,
            },
        })
    }
}
async fn load(
    lookup: &Lookup,
    backend: &Backend,
    key: &CacheKey,
    executor: &BackgroundExecutor,
) -> Option<Data> {
    match (lookup, backend, key) {
        (Lookup::Project(query), Backend::Remote(fs), CacheKey::Project { cwd, .. }) => Some(
            Data::Project(project_completions::remote(query.provider, cwd, fs).await),
        ),
        (Lookup::Project(query), Backend::Local { .. }, CacheKey::Project { cwd, .. }) => {
            let provider = query.provider;
            let cwd = PathBuf::from(cwd);
            Some(Data::Project(
                executor
                    .spawn(async move { project_completions::local(provider, &cwd) })
                    .await,
            ))
        }
        (Lookup::Paths(_), Backend::Remote(fs), CacheKey::Directory(directory)) => {
            Some(Data::Paths(
                fs.list_completion_dir(directory.clone())
                    .await
                    .ok()?
                    .into_iter()
                    .take(4096)
                    .map(|entry| Entry {
                        name: entry.name,
                        directory: entry.is_dir,
                    })
                    .collect(),
            ))
        }
        (Lookup::Paths(_), Backend::Local { .. }, CacheKey::Directory(directory)) => {
            let directory = directory.clone();
            Some(Data::Paths(
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
                    .await,
            ))
        }
        _ => None,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn caches_respect_source_directory_deadline_and_manual_refresh() {
        let key = CacheKey::Directory("/project".into());
        let mut cache = CacheEntry {
            key: key.clone(),
            loaded: Instant::now(),
            data: Data::Paths(Vec::new()),
        };
        assert!(cache.valid(&key, false));
        assert!(!cache.valid(&key, true));
        assert!(!cache.valid(&CacheKey::Directory("/other".into()), false));
        assert!(!cache.valid(
            &CacheKey::Project {
                provider: project_completions::Provider::NpmScripts,
                cwd: "/project".into()
            },
            false
        ));
        cache.loaded -= Duration::from_secs(4);
        assert!(!cache.valid(&key, false));
    }
}
