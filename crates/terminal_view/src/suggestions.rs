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
#[derive(Default)]
pub struct Suggestions {
    pub service: Option<std::sync::Arc<dyn crate::completion_service::CompletionService>>,
    pub settings: crate::SuggestionSettings,
    state: InputState,
    pub text: String,
    pub cursor: usize,
    pub revision: u64,
    candidate_revision: u64,
    history: Vec<String>,
    commands: Vec<String>,
    pub cwd: Option<String>,
    pub powershell: Option<bool>,
    pub candidates: Vec<Candidate>,
    pub selected: usize,
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
    pub fn clear_history(&mut self) {
        self.history.clear();
    }
    pub fn context(&mut self, key: &str, value: &str) {
        match key {
            "cwd" => self.cwd = (!value.is_empty()).then(|| value.into()),
            "shell" => self.powershell = Some(value == "powershell"),
            _ => {}
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
    pub fn merge_candidates(&mut self, candidates: Vec<Candidate>) {
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
                self.history.retain(|old| old != &command);
                self.history.insert(0, command);
                self.history.truncate(256);
                InputState::Inactive
            }
            _ => InputState::Inactive,
        };
        self.text.clear();
        self.cursor = 0;
        self.candidates.clear();
    }
    pub fn invalidate(&mut self) {
        self.state = InputState::Inactive;
        self.candidates.clear();
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
        }
    }
    pub fn provider_context(&self) -> crate::completion_service::Context<'_> {
        crate::completion_service::Context {
            text: &self.text,
            cursor: self.cursor,
            powershell: self.powershell,
            cwd: self.cwd.as_deref().unwrap_or(""),
            history: &self.history,
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
