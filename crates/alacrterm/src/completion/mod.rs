//! Completion providers, ranking policy, filesystem work and per-pane caches.
mod command_specs;
mod completion_provider;
mod completion_resources;
mod path_suggestions;
mod project_completions;
mod scoring;
use completion::{Candidate, rank};
use completion_provider::{Provider, Work};
use completion_resources::CacheEntry;
use settings::SuggestionSettings;
use std::sync::{Arc, Mutex};
use terminal_view::completion;
use terminal_view::completion_service::{Backend, CompletionJob, CompletionService, Context};

#[derive(Default)]
pub(crate) struct CompletionEngine {
    cache: Arc<Mutex<Cache>>,
}

#[derive(Default)]
struct Cache {
    generation: u64,
    entries: Vec<CacheEntry>,
}

fn context<'a>(query: &'a Context<'_>) -> completion_provider::Context<'a> {
    completion_provider::Context {
        text: query.text,
        cursor: query.cursor,
        powershell: query.powershell == Some(true),
        history: query.history,
        commands: query.commands,
        settings: query.settings,
        path: query.powershell.and_then(|powershell| {
            path_suggestions::PathQuery::parse(query.text, query.cursor, powershell)
        }),
    }
}

impl CompletionService for CompletionEngine {
    fn immediate(&self, query: &Context<'_>) -> Vec<Candidate> {
        let context = context(query);
        let mut candidates = Vec::new();
        for provider in completion_provider::IMMEDIATE {
            if let Some(Work::Ready(items)) = provider.query(&context) {
                candidates.extend(items);
            }
        }
        rank(&mut candidates);
        candidates
    }
    fn deferred(
        &self,
        query: &Context<'_>,
        backend: Backend,
        manual: bool,
    ) -> Option<Box<dyn CompletionJob>> {
        let context = context(query);
        let lookup = [Provider::Project, Provider::Paths]
            .into_iter()
            .find_map(|provider| match provider.query(&context) {
                Some(Work::Lookup(query)) => Some(query),
                _ => None,
            })?;
        let backend = match backend {
            Backend::Local => completion_resources::Backend::local(),
            Backend::Remote(fs) => completion_resources::Backend::Remote(fs),
        };
        let cache = self.cache.lock().unwrap_or_else(|error| error.into_inner());
        let generation = cache.generation;
        let request = completion_resources::Request {
            lookup,
            backend,
            cache: cache.entries.clone(),
            manual,
            cwd: query.cwd.into(),
        };
        Some(Box::new(Job {
            request,
            cache: self.cache.clone(),
            generation,
        }))
    }
    fn filter_text(&self, query: &Context<'_>, candidate: &Candidate) -> String {
        if candidate.source == completion::Source::Filesystem {
            return context(query).path.map_or(String::new(), |path| path.leaf);
        }
        query
            .text
            .get(candidate.replacement.start..query.cursor)
            .unwrap_or("")
            .into()
    }
    fn reset(&self) {
        let mut cache = self.cache.lock().unwrap_or_else(|error| error.into_inner());
        cache.generation = cache.generation.wrapping_add(1);
        cache.entries.clear();
    }
}

struct Job {
    generation: u64,
    request: completion_resources::Request,
    cache: Arc<Mutex<Cache>>,
}
impl CompletionJob for Job {
    fn resolve(
        self: Box<Self>,
        executor: gpui::BackgroundExecutor,
    ) -> futures::future::BoxFuture<'static, Vec<Candidate>> {
        Box::pin(async move {
            let Some(response) = self.request.resolve(executor).await else {
                return Vec::new();
            };
            let mut cache = self.cache.lock().unwrap_or_else(|error| error.into_inner());
            if cache.generation != self.generation {
                return Vec::new();
            }
            cache
                .entries
                .retain(|entry| !entry.key.same_source(&response.cache.key));
            cache.entries.push(response.cache);
            response.candidates
        })
    }
}

#[cfg(test)]
mod tests;
