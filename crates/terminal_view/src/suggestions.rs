//! Per-pane, memory-only history suggestions gated by shell lifecycle markers.
use crate::completion::{Candidate, Kind, rank};
use terminal::PromptEvent;
/// A submitted command cannot simultaneously be an editable input line.
#[derive(Default)]
enum InputState {
    #[default]
    Inactive,
    Prompt,
    Editing {
        menu: MenuState,
    },
    Submitted {
        command: Option<String>,
    },
}
#[derive(Clone, Copy)]
enum MenuState {
    Ready,
    Dismissed,
}
struct CachedInput {
    text: String,
    cursor: usize,
    cwd: Option<String>,
    powershell: Option<bool>,
}
struct CachedCandidate {
    item: Candidate,
    input: std::sync::Arc<CachedInput>,
}
#[derive(Default)]
pub struct Suggestions {
    pub service: Option<std::sync::Arc<dyn crate::completion_service::CompletionService>>,
    pub settings: crate::SuggestionSettings,
    state: InputState,
    pub text: String,
    pub cursor: usize,
    pub revision: u64,
    candidate_revision: u64,
    history: crate::history::History,
    commands: Vec<String>,
    pub cwd: Option<String>,
    pub powershell: Option<bool>,
    pub candidates: Vec<Candidate>,
    pub selected: usize,
    cached: Vec<CachedCandidate>,
}
impl Suggestions {
    pub fn new(settings: crate::SuggestionSettings) -> Self {
        Self {
            settings,
            ..Default::default()
        }
    }
    pub fn is_editing(&self) -> bool {
        matches!(self.state, InputState::Editing { .. })
    }
    fn menu_dismissed(&self) -> bool {
        matches!(
            self.state,
            InputState::Editing {
                menu: MenuState::Dismissed
            }
        )
    }
    fn resume_menu(&mut self) {
        if let InputState::Editing { menu } = &mut self.state {
            *menu = MenuState::Ready;
        }
    }
    pub fn import_history(&mut self, event: terminal::HistoryEvent) -> bool {
        let visible = self.has_current_candidates();
        let selected = self
            .candidates
            .get(self.selected)
            .map(|item| item.insert_text.clone());
        if !self
            .history
            .receive(event, self.settings.enabled && self.settings.history)
        {
            return false;
        }
        self.revision = self.revision.wrapping_add(1);
        self.clear_cache();
        if visible {
            self.refresh_in_place(false, false);
            if let Some(index) = self
                .candidates
                .iter()
                .position(|item| Some(&item.insert_text) == selected.as_ref())
            {
                self.selected = index;
            }
        }
        true
    }
    pub fn reset_history(&mut self) {
        self.history.reset();
    }
    pub fn clear_history(&mut self) {
        self.history.clear();
    }
    pub fn clear_cache(&mut self) {
        self.cached.clear();
        self.candidates.clear();
    }
    pub fn context(&mut self, key: &str, value: &str) {
        let changed = match key {
            "cwd" => {
                let next = (!value.is_empty()).then(|| value.into());
                let changed = self.cwd != next;
                self.cwd = next;
                changed
            }
            "shell" => {
                let next = Some(value == "powershell");
                let changed = self.powershell != next;
                self.powershell = next;
                changed
            }
            _ => false,
        };
        if changed {
            self.revision = self.revision.wrapping_add(1);
            self.clear_cache();
        }
    }
    pub fn can_query(&self) -> bool {
        self.is_editing() && !self.menu_dismissed() && self.settings.enabled
    }
    pub fn has_current_candidates(&self) -> bool {
        self.can_query()
            && self.candidate_revision == self.revision
            && self.selected < self.candidates.len()
    }
    fn remember(&mut self, items: &[Candidate]) {
        if items.is_empty() {
            return;
        }
        let input = std::sync::Arc::new(CachedInput {
            text: self.text.clone(),
            cursor: self.cursor,
            cwd: self.cwd.clone(),
            powershell: self.powershell,
        });
        let mut cached: Vec<_> = items
            .iter()
            .cloned()
            .map(|item| CachedCandidate {
                item,
                input: input.clone(),
            })
            .collect();
        cached.append(&mut self.cached);
        let mut seen = std::collections::HashSet::new();
        cached.retain(|entry| {
            seen.insert((
                entry.item.insert_text.clone(),
                entry.item.replacement.start,
                entry.item.source as u8,
            ))
        });
        cached.truncate(4096);
        self.cached = cached;
    }

    fn cached_matches(&self) -> Vec<Candidate> {
        let Some(service) = &self.service else {
            return Vec::new();
        };
        let current = self.provider_context();
        self.cached
            .iter()
            .filter_map(|entry| {
                let previous = crate::completion_service::Context {
                    text: &entry.input.text,
                    cursor: entry.input.cursor,
                    cwd: entry.input.cwd.as_deref().unwrap_or(""),
                    powershell: entry.input.powershell,
                    ..self.provider_context()
                };
                service.reuse_candidate(&previous, &current, &entry.item)
            })
            .collect()
    }

    fn replace_candidates(&mut self, mut items: Vec<Candidate>) {
        let selected = self
            .candidates
            .get(self.selected)
            .map(|item| item.insert_text.clone());
        rank(&mut items);
        self.selected = selected
            .and_then(|text| items.iter().position(|item| item.insert_text == text))
            .unwrap_or(0);
        self.candidates = items;
        self.candidate_revision = self.revision;
    }

    /// Deletion refilters only the session cache, never calls providers.
    pub fn refilter_cached(&mut self) {
        if self.text.is_empty() || !self.can_query() {
            self.dismiss();
            return;
        }
        self.replace_candidates(self.cached_matches());
    }

    pub fn refresh_in_place(&mut self, manual: bool, pending: bool) {
        if !self.can_query() || (!manual && (!self.settings.automatic || self.text.is_empty())) {
            self.candidates.clear();
            return;
        }
        let immediate = self
            .service
            .as_ref()
            .map(|service| service.immediate(&self.provider_context()))
            .unwrap_or_default();
        let mut items = self.cached_matches();
        // Fresh provider scores/ranges win over reused items with the same insertion.
        let fresh: std::collections::HashSet<_> = immediate
            .iter()
            .map(|item| {
                (
                    item.insert_text.clone(),
                    item.replacement.start,
                    item.replacement.end,
                )
            })
            .collect();
        items.retain(|old| {
            !fresh.contains(&(
                old.insert_text.clone(),
                old.replacement.start,
                old.replacement.end,
            ))
        });
        items.extend(immediate.clone());
        self.remember(&immediate);
        if !items.is_empty() || !pending {
            self.replace_candidates(items);
        }
        // When empty and pending, previous display items remain stale and cannot be accepted.
    }

    pub fn replace_deferred(&mut self, candidates: Vec<Candidate>) {
        let selected = self
            .candidates
            .get(self.selected)
            .map(|item| item.insert_text.clone());
        self.candidates.retain(|item| {
            !matches!(
                item.source,
                crate::completion::Source::Filesystem | crate::completion::Source::Project
            )
        });
        self.merge_candidates(candidates);
        if let Some(index) = selected.and_then(|text| {
            self.candidates
                .iter()
                .position(|item| item.insert_text == text)
        }) {
            self.selected = index;
        }
    }
    pub fn merge_candidates(&mut self, candidates: Vec<Candidate>) {
        self.remember(&candidates);
        if self.candidate_revision != self.revision {
            self.candidates.clear();
        }
        self.candidate_revision = self.revision;
        let selected = self
            .candidates
            .get(self.selected)
            .map(|item| (item.insert_text.clone(), item.replacement.clone()));
        self.candidates.extend(candidates);
        rank(&mut self.candidates);
        if let Some((text, range)) = selected {
            self.selected = self
                .candidates
                .iter()
                .position(|item| item.insert_text == text && item.replacement == range)
                .unwrap_or(0);
        }
    }
    pub fn add_command(&mut self, name: &str) {
        if name.is_empty() {
            self.commands.clear();
            return;
        }
        if self.commands.len() >= 4096 {
            return;
        }
        if let Err(index) = self.commands.binary_search_by(|old| old.as_str().cmp(name)) {
            self.commands.insert(index, name.to_string());
        }
    }
    pub fn phase(&mut self, event: PromptEvent) {
        if event != PromptEvent::Input {
            self.cached.clear();
        }
        self.revision = self.revision.wrapping_add(1);
        if event == PromptEvent::Input && self.is_editing() && self.text.is_empty() {
            return;
        }
        let previous = std::mem::take(&mut self.state);
        self.state = match (event, previous) {
            (PromptEvent::Prompt, _) => InputState::Prompt,
            (PromptEvent::Input, InputState::Prompt) => InputState::Editing {
                menu: MenuState::Ready,
            },
            (
                PromptEvent::Execute,
                InputState::Submitted {
                    command: Some(command),
                },
            ) if self.settings.history => {
                self.history.record(command);
                InputState::Inactive
            }
            _ => InputState::Inactive,
        };
        self.text.clear();
        self.cursor = 0;
        self.candidates.clear();
    }
    pub fn invalidate(&mut self) {
        self.cached.clear();
        self.state = InputState::Inactive;
        self.candidates.clear();
    }
    pub fn suspend_for_recovery(&mut self) {
        self.state = InputState::Inactive;
        self.revision = self.revision.wrapping_add(1);
    }
    pub fn synchronize(&mut self, text: String, cursor: usize) {
        if text.len() > 4096 || text.chars().any(char::is_control) || !text.is_char_boundary(cursor)
        {
            return;
        }
        self.revision = self.revision.wrapping_add(1);
        self.text = text;
        self.cursor = cursor;
        self.state = InputState::Editing {
            menu: MenuState::Ready,
        };
        self.candidates.clear();
    }
    pub fn input(&mut self, text: &str) {
        if !self.is_editing() {
            return;
        }
        if text.chars().any(char::is_control) || self.text.len() + text.len() > 4096 {
            self.invalidate();
            return;
        }
        self.revision = self.revision.wrapping_add(1);
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.resume_menu();
    }
    pub fn submit(&mut self) {
        let command = (self.is_editing()
            && !self.text.trim().is_empty()
            && !self.text.starts_with(char::is_whitespace))
        .then(|| self.text.clone());
        self.state = InputState::Submitted { command };
        self.candidates.clear();
    }
    pub fn edit_key(&mut self, key: &str, control: bool) {
        self.revision = self.revision.wrapping_add(1);
        if !self.is_editing() {
            return;
        }
        self.resume_menu();
        match (key, control) {
            ("left", false) => {
                self.cursor = self.text[..self.cursor]
                    .char_indices()
                    .next_back()
                    .map(|(at, _)| at)
                    .unwrap_or(0)
            }
            ("right", false) => {
                if let Some(ch) = self.text[self.cursor..].chars().next() {
                    self.cursor += ch.len_utf8();
                }
            }
            ("home", false) | ("a", true) => self.cursor = 0,
            ("end", false) | ("e", true) => self.cursor = self.text.len(),
            ("u", true) if self.powershell != Some(true) => {
                self.text.drain(..self.cursor);
                self.cursor = 0;
            }
            ("k", true) if self.powershell != Some(true) => {
                self.text.truncate(self.cursor);
            }
            ("w", true) if self.powershell != Some(true) => {
                let prefix = &self.text[..self.cursor];
                let end = prefix.trim_end_matches(char::is_whitespace).len();
                let start = prefix[..end]
                    .rfind(char::is_whitespace)
                    .map_or(0, |at| at + prefix[at..].chars().next().unwrap().len_utf8());
                self.text.drain(start..self.cursor);
                self.cursor = start;
            }
            ("backspace", false) if self.cursor > 0 => {
                let start = self.text[..self.cursor]
                    .char_indices()
                    .next_back()
                    .map(|(at, _)| at)
                    .unwrap_or(0);
                self.text.drain(start..self.cursor);
                self.cursor = start;
            }
            ("delete", false) => {
                if let Some(ch) = self.text[self.cursor..].chars().next() {
                    self.text.drain(self.cursor..self.cursor + ch.len_utf8());
                }
            }
            _ => self.invalidate(),
        }
    }
    pub fn refresh(&mut self, manual: bool) {
        self.candidates.clear();
        self.candidate_revision = self.revision;
        self.selected = 0;
        if !self.is_editing()
            || self.menu_dismissed()
            || !self.settings.enabled
            || (!manual && (!self.settings.automatic || self.text.is_empty()))
        {
            return;
        }
        if let Some(service) = &self.service {
            self.candidates = service.immediate(&self.provider_context());
            self.remember(&self.candidates.clone());
        }
    }
    pub fn provider_context(&self) -> crate::completion_service::Context<'_> {
        crate::completion_service::Context {
            text: &self.text,
            cursor: self.cursor,
            powershell: self.powershell,
            cwd: self.cwd.as_deref().unwrap_or(""),
            history: self.history.entries(),
            commands: &self.commands,
            settings: &self.settings,
        }
    }
    pub fn query_for(&self, candidate: &Candidate) -> String {
        if let Some(service) = &self.service {
            return service.filter_text(&self.provider_context(), candidate);
        }
        self.text
            .get(candidate.replacement.start..self.cursor)
            .unwrap_or("")
            .to_string()
    }
    pub fn trigger(&mut self) {
        self.resume_menu();
        self.refresh(true);
    }
    pub fn dismiss(&mut self) {
        self.candidates.clear();
        if let InputState::Editing { menu } = &mut self.state {
            *menu = MenuState::Dismissed;
        }
    }
    pub fn step(&mut self, delta: isize) {
        if !self.candidates.is_empty() {
            self.selected = (self.selected as isize + delta)
                .rem_euclid(self.candidates.len() as isize) as usize;
        }
    }
    /// Enter can submit an already complete default candidate without inserting
    /// the optional trailing space or changing the shell's cursor position.
    pub fn default_candidate_matches_input(&self) -> bool {
        if !self.has_current_candidates() || self.selected != 0 || self.cursor != self.text.len() {
            return false;
        }
        let item = &self.candidates[0];
        item.cursor_back == 0
            && item.replacement.end == self.cursor
            && self.text.get(item.replacement.clone()) == Some(item.insert_text.as_str())
    }

    pub fn accept(&mut self) -> Option<String> {
        if !self.is_editing() || self.candidate_revision != self.revision {
            return None;
        }
        let mut item = self.candidates.get(self.selected)?.clone();
        if self.settings.insert_trailing_space
            && !item.retrigger
            && item.replacement.end == self.text.len()
            && item.kind != Kind::History
        {
            item.insert_text.push(' ');
        }
        let range = item.replacement.clone();
        if self.cursor < range.start || self.cursor > range.end {
            return None;
        }
        let old = self.text.get(range.clone())?;
        if item.insert_text.chars().any(char::is_control) {
            return None;
        }
        let mut bytes = if self.cursor == range.end && item.insert_text.starts_with(old) {
            item.insert_text[old.len()..].to_string()
        } else {
            // Non-ASCII line editors disagree about combining-character movement.
            // Keep their native behavior unless accepting a suffix requires no edit.
            if !old.is_ascii() {
                return None;
            }
            format!(
                "{}{}{}",
                "\x1b[D".repeat(self.cursor - range.start),
                "\x1b[3~".repeat(old.len()),
                item.insert_text
            )
        };
        bytes.push_str(&"\x1b[D".repeat(item.cursor_back));
        let new_cursor = range.start + item.insert_text.len() - item.cursor_back;
        self.text.replace_range(range, &item.insert_text);
        self.cursor = new_cursor;
        self.revision = self.revision.wrapping_add(1);
        self.dismiss();
        Some(bytes)
    }
}
