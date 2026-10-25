//! Per-pane, memory-only history suggestions gated by shell lifecycle markers.
use crate::completion::{Candidate, Kind, Source, rank};
use terminal::PromptEvent;
#[derive(Default)]
pub(super) struct Suggestions {
    pub active: bool,
    pub text: String,
    cursor: usize,
    history: Vec<String>,
    commands: Vec<String>,
    pub cwd: Option<String>,
    pub powershell: Option<bool>,
    pending: Option<String>,
    pub candidates: Vec<Candidate>,
    pub selected: usize,
    dismissed: bool,
    prompt_ready: bool,
}
impl Suggestions {
    pub fn context(&mut self, key: &str, value: &str) {
        match key {
            "cwd" => self.cwd = (!value.is_empty()).then(|| value.into()),
            "shell" => self.powershell = Some(value == "powershell"),
            _ => {}
        }
    }
    pub fn path_query(&self) -> Option<crate::path_suggestions::PathQuery> {
        if !self.active || self.dismissed || self.cursor != self.text.len() {
            return None;
        }
        crate::path_suggestions::PathQuery::parse(&self.text, self.powershell?)
    }
    pub fn add_paths(&mut self, candidates: Vec<String>) {
        let selected = self
            .candidates
            .get(self.selected)
            .map(|item| (item.insert_text.clone(), item.replacement.clone()));
        for text in candidates {
            if text.starts_with(&self.text) {
                let directory = text.ends_with('/');
                let kind = if directory {
                    Kind::Directory
                } else {
                    Kind::File
                };
                self.candidates.push(Candidate::new(
                    text,
                    self.text.len(),
                    kind,
                    Source::Filesystem,
                    if directory { 1600 } else { 1500 },
                ));
            }
        }
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
        if event == PromptEvent::Input && self.active && self.text.is_empty() {
            return;
        }
        if event == PromptEvent::Execute {
            if let Some(command) = self.pending.take() {
                self.history.retain(|old| old != &command);
                self.history.insert(0, command);
                self.history.truncate(256);
            }
        } else {
            self.pending = None;
        }
        self.active = event == PromptEvent::Input && self.prompt_ready;
        self.prompt_ready = event == PromptEvent::Prompt;
        self.text.clear();
        self.cursor = 0;
        self.dismiss();
        self.dismissed = false;
    }
    pub fn invalidate(&mut self) {
        self.active = false;
        self.pending = None;
        self.dismiss();
    }
    pub fn input(&mut self, text: &str) {
        if !self.active {
            return;
        }
        if text.chars().any(char::is_control) || self.text.len() + text.len() > 4096 {
            self.invalidate();
            return;
        }
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.dismissed = false;
    }
    pub fn submit(&mut self) {
        self.pending = (self.active
            && !self.text.trim().is_empty()
            && !self.text.starts_with(char::is_whitespace))
        .then(|| self.text.clone());
        self.active = false;
        self.dismiss();
    }
    pub fn edit_key(&mut self, key: &str, control: bool) {
        if !self.active {
            return;
        }
        self.dismissed = false;
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
        self.selected = 0;
        if !self.active
            || self.dismissed
            || self.cursor != self.text.len()
            || (!manual && self.text.is_empty())
        {
            return;
        }
        self.candidates.extend(
            self.history
                .iter()
                .enumerate()
                .filter(|(_, command)| command.starts_with(&self.text) && **command != self.text)
                .map(|(index, command)| {
                    Candidate::new(
                        command.clone(),
                        self.text.len(),
                        Kind::History,
                        Source::History,
                        1800 - index as i64,
                    )
                }),
        );
        if !self.text.chars().any(char::is_whitespace) {
            self.candidates.extend(
                self.commands
                    .iter()
                    .filter(|command| command.starts_with(&self.text) && **command != self.text)
                    .map(|command| {
                        Candidate::new(
                            command.clone(),
                            self.text.len(),
                            Kind::Command,
                            Source::Shell,
                            1700,
                        )
                    }),
            );
        }
        let command = self.text.split_whitespace().next().unwrap_or("");
        if self
            .commands
            .binary_search_by(|name| name.as_str().cmp(command))
            .is_ok()
        {
            for (text, description) in crate::command_specs::candidates(&self.text) {
                let kind = if text
                    .split_whitespace()
                    .last()
                    .is_some_and(|word| word.starts_with('-'))
                {
                    Kind::Option
                } else {
                    Kind::Subcommand
                };
                let mut item =
                    Candidate::new(text, self.text.len(), kind, Source::Specification, 2000);
                item.description = Some(description.into());
                self.candidates.push(item);
            }
        }
        rank(&mut self.candidates);
    }
    pub fn trigger(&mut self) {
        self.dismissed = false;
        self.refresh(true);
    }
    pub fn dismiss(&mut self) {
        self.candidates.clear();
        self.dismissed = true;
    }
    pub fn step(&mut self, delta: isize) {
        if !self.candidates.is_empty() {
            self.selected = (self.selected as isize + delta)
                .rem_euclid(self.candidates.len() as isize) as usize;
        }
    }
    pub fn accept(&mut self) -> Option<String> {
        if !self.active || self.cursor != self.text.len() {
            return None;
        }
        let candidate = self.candidates.get(self.selected)?;
        if candidate.replacement != (0..self.text.len()) {
            return None;
        }
        let suffix = self
            .candidates
            .get(self.selected)?
            .insert_text
            .strip_prefix(&self.text)?
            .to_string();
        self.input(&suffix);
        self.dismiss();
        Some(suffix)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn texts(state: &Suggestions) -> Vec<&str> {
        state
            .candidates
            .iter()
            .map(|item| item.insert_text.as_str())
            .collect()
    }
    fn execute(state: &mut Suggestions, command: &str) {
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.input(command);
        state.submit();
        state.phase(PromptEvent::Execute);
    }
    #[test]
    fn providers_merge_without_starvation_and_keep_selection_on_late_results() {
        let mut state = Suggestions::default();
        for index in 0..20 {
            execute(&mut state, &format!("git status{index}"));
        }
        state.add_command("git");
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.input("git sta");
        state.refresh(false);
        assert_eq!(state.candidates[0].kind, Kind::Subcommand);
        assert_eq!(state.candidates.len(), 21);
        state.step(3);
        let selected = state.candidates[state.selected].insert_text.clone();
        state.add_paths(vec!["git station/".into(), "git status".into()]);
        assert_eq!(state.candidates[state.selected].insert_text, selected);
        assert_eq!(
            state
                .candidates
                .iter()
                .filter(|item| item.insert_text == "git status")
                .count(),
            1
        );
        assert!(
            state
                .candidates
                .iter()
                .any(|item| item.kind == Kind::Directory)
        );
    }
    #[test]
    fn all_candidates_remain_navigable_beyond_the_visible_rows() {
        let mut state = Suggestions::default();
        for index in 0..40 {
            state.add_command(&format!("demo{index:02}"));
        }
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.input("demo");
        state.refresh(false);
        assert_eq!(state.candidates.len(), 40);
        for _ in 0..12 {
            state.step(1);
        }
        assert_eq!(state.accept().as_deref(), Some("12"));
        assert_eq!(state.text, "demo12");
    }
    #[test]
    fn command_specs_require_current_shell_inventory() {
        let mut state = Suggestions::default();
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.input("git sta");
        state.refresh(false);
        assert!(state.candidates.is_empty());
        state.add_command("git");
        state.refresh(false);
        assert_eq!(texts(&state), ["git status"]);
        assert_eq!(
            state.candidates[0].description.as_deref(),
            Some("查看工作区状态")
        );
        assert_eq!(state.accept().as_deref(), Some("tus"));
        assert!(state.active);
    }
    #[test]
    fn only_confirmed_commands_become_history_and_accept_never_submits() {
        let mut state = Suggestions::default();
        state.input("secret-password");
        state.submit();
        state.phase(PromptEvent::Execute);
        execute(&mut state, "git status");
        execute(&mut state, "git log");
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.input("git ");
        state.refresh(false);
        assert_eq!(texts(&state), ["git log", "git status"]);
        state.step(1);
        assert_eq!(state.accept().as_deref(), Some("status"));
        assert!(state.active);
        assert_eq!(state.text, "git status");
        assert!(state.pending.is_none());
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.trigger();
        assert!(
            !state
                .candidates
                .iter()
                .any(|line| line.insert_text.contains("secret"))
        );
    }
    #[test]
    fn unicode_edits_and_unknown_shell_operations_do_not_corrupt_insertion() {
        let mut state = Suggestions::default();
        execute(&mut state, "echo 中文");
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.input("echo 中");
        state.edit_key("backspace", false);
        assert_eq!(state.text, "echo ");
        state.refresh(false);
        assert_eq!(state.accept().as_deref(), Some("中文"));
        state.edit_key("up", false);
        state.trigger();
        assert!(state.candidates.is_empty());
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.input("echo\npassword");
        assert!(!state.active);
    }
    #[test]
    fn redraw_and_private_history_entries_do_not_create_candidates() {
        let mut state = Suggestions::default();
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.phase(PromptEvent::Input);
        assert!(state.active);
        state.input("echo partial");
        state.phase(PromptEvent::Input);
        assert!(!state.active);
        execute(&mut state, " secret-token");
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.trigger();
        assert!(state.candidates.is_empty());
    }
    #[test]
    fn shell_commands_merge_with_history_and_reset_per_shell() {
        let mut state = Suggestions::default();
        state.add_command("git");
        state.add_command("grep");
        state.add_command("git");
        execute(&mut state, "git status");
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.input("g");
        state.refresh(false);
        assert_eq!(texts(&state), ["git status", "git", "grep"]);
        state.add_command("");
        state.add_command("remote-command");
        state.refresh(false);
        assert_eq!(texts(&state), ["git status"]);
        state.input("it ");
        state.refresh(false);
        assert_eq!(texts(&state), ["git status"]);
    }
}
