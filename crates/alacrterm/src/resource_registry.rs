//! GPUI adapter for the framework-independent resource registry.
use anyhow::Result;
use gpui::{App, Global, Task};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use util::resource_registry::{DirectoryWatch, Resource};

pub(crate) struct ResourceRegistry<T: 'static> {
    resources: util::resource_registry::ResourceRegistry<T>,
    watcher: Option<DirectoryWatch<T>>,
    task: Option<Task<()>>,
    generation: u64,
}
impl<T: 'static> Global for ResourceRegistry<T> {}
impl<T: PartialEq + Send + Sync + 'static> ResourceRegistry<T> {
    pub fn init(cx: &mut App) {
        cx.set_global(Self {
            resources: Default::default(),
            watcher: None,
            task: None,
            generation: 0,
        });
    }
    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }
    pub fn global_mut(cx: &mut App) -> &mut Self {
        cx.global_mut::<Self>()
    }
    pub fn register_defaults(&mut self, entries: impl IntoIterator<Item = Resource<T>>) {
        self.resources.register_defaults(entries);
    }
    pub fn get(&self, id: &str) -> Option<Arc<T>> {
        self.resources.get(id)
    }
    pub fn entries(&self) -> &std::collections::HashMap<String, Arc<T>> {
        self.resources.entries()
    }
    pub fn watch_dir(
        path: PathBuf,
        loader: impl Fn(&Path, &str) -> Result<Resource<T>> + Send + 'static,
        cx: &mut App,
    ) -> Result<()> {
        let watcher = DirectoryWatch::new(path, loader)?;
        let updates = watcher.updates();
        let registry = Self::global_mut(cx);
        registry.task = None;
        registry.watcher = Some(watcher);
        registry.generation = registry.generation.wrapping_add(1);
        let generation = registry.generation;
        let task = cx.spawn(async move |cx| {
            while let Ok(snapshot) = updates.recv().await {
                cx.update(|cx| {
                    if Self::global(cx).generation == generation {
                        Self::global_mut(cx).resources.apply(snapshot);
                    }
                });
            }
        });
        Self::global_mut(cx).task = Some(task);
        Ok(())
    }
}
