//! Per-pane, memory-only history suggestions gated by shell lifecycle markers.
use crate::completion::{Candidate, Kind, Source, match_score, rank};
use terminal::PromptEvent;
#[derive(Default)]
pub(super) struct Suggestions {
    pub settings: crate::SuggestionSettings,
    pub active: bool,
    pub text: String,
    pub cursor: usize,
    pub revision: u64,
    candidate_revision: u64,
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
    pub fn new(settings: crate::SuggestionSettings) -> Self {
        Self {
            settings,
            ..Default::default()
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
    pub fn path_query(&self) -> Option<crate::path_suggestions::PathQuery> {
        if !self.active || self.dismissed || !self.settings.enabled || !(self.settings.paths || self.settings.dynamic) {
            return None;
        }
        crate::path_suggestions::PathQuery::parse(&self.text, self.cursor, self.powershell?)
    }
    pub fn add_paths(&mut self, candidates: Vec<Candidate>) {
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
    pub fn has_command(&self, name: &str) -> bool { self.commands.binary_search_by(|command| command.as_str().cmp(name)).is_ok() }
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
        if event == PromptEvent::Input && self.active && self.text.is_empty() {
            return;
        }
        if event == PromptEvent::Execute && self.settings.history {
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
    pub fn synchronize(&mut self, text: String, cursor: usize) {
        if text.len() > 4096 || text.chars().any(char::is_control) || !text.is_char_boundary(cursor)
        {
            return;
        }
        self.revision = self.revision.wrapping_add(1);
        self.text = text;
        self.cursor = cursor;
        self.active = true;
        self.pending = None;
        self.dismissed = false;
        self.candidates.clear();
    }
    pub fn input(&mut self, text: &str) {
        if !self.active {
            return;
        }
        if text.chars().any(char::is_control) || self.text.len() + text.len() > 4096 {
            self.invalidate();
            return;
        }
        self.revision = self.revision.wrapping_add(1);
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
        self.revision = self.revision.wrapping_add(1);
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
        self.candidate_revision = self.revision;
        self.selected = 0;
        if !self.active
            || self.dismissed
            || !self.settings.enabled
            || (!manual && (!self.settings.automatic || self.text.is_empty()))
        {
            return;
        }
        if self.cursor == self.text.len() {
            self.candidates.extend(
                self.history
                    .iter()
                    .enumerate()
                    .filter(|(_, command)| {
                        self.settings.history
                            && command.starts_with(&self.text)
                            && **command != self.text
                    })
                    .map(|(index, command)| {
                        Candidate::new(
                            command.clone(),
                            self.text.len(),
                            Kind::History,
                            Source::History,
                            1100 - (index as i64).min(100),
                        )
                    }),
            );
            let command = self.text.split_whitespace().next().unwrap_or("");
            if self.settings.specifications
                && self
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
                        Candidate::new(text, self.text.len(), kind, Source::Specification, 1250);
                    item.description = Some(description.into());
                    self.candidates.push(item);
                }
            }
        }
        let word_end = self
            .text
            .find(char::is_whitespace)
            .unwrap_or(self.text.len());
        if self.settings.commands
            && self.cursor <= word_end
            && self.text[..word_end]
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || "_.:+-".contains(ch))
        {
            let prefix = &self.text[..self.cursor];
            for command in &self.commands {
                if let Some(score) = match_score(
                    prefix,
                    command,
                    self.powershell == Some(true),
                    self.settings.fuzzy,
                ) {
                    let mut item =
                        Candidate::new(command.clone(), 0, Kind::Command, Source::Shell, score);
                    item.replacement = 0..word_end;
                    self.candidates.push(item);
                }
            }
        }
        rank(&mut self.candidates);
    }
    pub fn query_for(&self, candidate: &Candidate) -> String {
        if candidate.source == Source::Filesystem {
            return self.path_query().map_or(String::new(), |query| query.leaf);
        }
        self.text
            .get(candidate.replacement.start..self.cursor)
            .unwrap_or("")
            .to_string()
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
        if !self.active || self.candidate_revision != self.revision {
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
        state.add_paths(vec![
            Candidate::new(
                "git station/".into(),
                state.text.len(),
                Kind::Directory,
                Source::Filesystem,
                1950,
            ),
            Candidate::new(
                "git status".into(),
                state.text.len(),
                Kind::File,
                Source::Filesystem,
                1900,
            ),
        ]);
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
    fn replacement_preserves_following_arguments_and_rejects_stale_candidates() {
        let mut state = Suggestions::default();
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.context("shell", "posix");
        state.input("cat saZZ other");
        for _ in 0..8 {
            state.edit_key("left", false);
        }
        state.refresh(false);
        let query = state.path_query().unwrap();
        state.add_paths(query.candidates(vec![crate::path_suggestions::Entry {
            name: "sample file".into(),
            directory: false,
        }]));
        let edit = state.accept().unwrap();
        assert_eq!(state.text, "cat sample\\ file other");
        assert!(!edit.contains('\n'));
        state.refresh(false);
        state.input("x");
        assert!(state.accept().is_none());
    }
    #[test]
    fn fuzzy_and_middle_command_completion_replace_only_the_command_word() {
        let mut state = Suggestions::default();
        state.add_command("git");
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.synchronize("gt status".into(), 2);
        state.refresh(false);
        assert_eq!(state.candidates[0].replacement, 0..2);
        assert!(state.accept().unwrap().contains("git"));
        assert_eq!(state.text, "git status");
        state.synchronize("get-i".into(), 5);
        state.context("shell", "powershell");
        state.add_command("Get-Item");
        state.refresh(false);
        assert_eq!(state.candidates[0].insert_text, "Get-Item");
    }
    #[test]
    fn policy_controls_manual_trigger_sources_fuzzy_matching_and_trailing_space() {
        let mut state = Suggestions::default();
        state.add_command("git");
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.input("gt");
        state.settings.automatic = false;
        state.refresh(false);
        assert!(state.candidates.is_empty());
        state.refresh(true);
        assert_eq!(state.candidates[0].insert_text, "git");
        state.settings.fuzzy = false;
        state.refresh(true);
        assert!(state.candidates.is_empty());
        state.settings.fuzzy = true;
        state.settings.commands = false;
        state.refresh(true);
        assert!(state.candidates.is_empty());
        state.settings.commands = true;
        state.settings.insert_trailing_space = true;
        state.refresh(true);
        state.accept().unwrap();
        assert_eq!(state.text, "git ");
        state.settings.enabled = false;
        state.refresh(true);
        assert!(state.candidates.is_empty());
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
