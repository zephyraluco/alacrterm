use std::sync::Arc;
use terminal::PromptEvent;
use terminal_view::SuggestionModel as Suggestions;
use terminal_view::completion::{Candidate, Kind, Source};
fn state() -> Suggestions {
    let mut state = Suggestions::default();
    state.service = Some(Arc::new(super::CompletionEngine::default()));
    state
}
fn path_query(state: &Suggestions) -> Option<super::path_suggestions::PathQuery> {
    super::path_suggestions::PathQuery::parse(&state.text, state.cursor, state.powershell?)
}
mod behavior {
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
        import_command(state, command);
    }
    fn import_command(state: &mut Suggestions, command: &str) {
        use terminal::HistoryEvent;
        static BATCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let batch = BATCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        state.import_history(HistoryEvent::Session("test-shell".into()));
        state.import_history(HistoryEvent::Begin {
            session: "test-shell".into(),
            batch,
            reset: false,
        });
        state.import_history(HistoryEvent::Entry {
            session: "test-shell".into(),
            batch,
            index: 1,
            command: command.into(),
        });
        state.import_history(HistoryEvent::End {
            session: "test-shell".into(),
            batch,
            count: 1,
        });
    }
    #[test]
    fn exact_commands_remain_visible_rank_first_and_accept_without_execution() {
        let mut state = state();
        for command in ["git", "gitk"] {
            state.add_command(command);
        }
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.context("shell", "posix");
        state.input("git");
        state.refresh(false);
        assert_eq!(texts(&state), ["git", "gitk"]);
        assert_eq!(state.accept().as_deref(), Some(""));
        assert_eq!(state.text, "git");
        assert!(!state.has_current_candidates());

        state.synchronize("git status".into(), 10);
        state.refresh(false);
        assert_eq!(texts(&state), ["git status"]);
        assert_eq!(state.accept().as_deref(), Some(""));
    }

    #[test]
    fn exact_history_survives_source_deduplication_and_outranks_longer_entries() {
        let mut state = state();
        execute(&mut state, "echo hello");
        execute(&mut state, "echo hello world");
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.input("echo hello");
        state.refresh(false);
        assert_eq!(texts(&state), ["echo hello", "echo hello world"]);

        state.add_command("git");
        execute(&mut state, "git");
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.input("git");
        state.refresh(false);
        assert_eq!(texts(&state), ["git"]);
    }

    #[test]
    fn interrupted_submission_and_out_of_order_markers_never_enter_history() {
        let mut state = state();
        state.phase(PromptEvent::Input);
        assert!(!state.is_editing());
        state.input("untracked");
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.input("echo unconfirmed");
        state.submit();
        state.phase(PromptEvent::Finished);
        state.phase(PromptEvent::Execute);
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.trigger();
        assert!(state.candidates.is_empty());
    }
    #[test]
    fn dismiss_keeps_input_editable_but_submit_stops_capturing_password_text() {
        let mut state = state();
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.input("echo hi");
        state.dismiss();
        assert!(state.is_editing());
        state.input("x");
        state.submit();
        state.input("password-value");
        assert_eq!(state.text, "echo hix");
        state.phase(PromptEvent::Execute);
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.trigger();
        assert!(texts(&state).is_empty()); // Keyboard submission alone never records history.
    }
    #[test]
    fn providers_merge_without_starvation_and_keep_selection_on_late_results() {
        let mut state = state();
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
        state.merge_candidates(vec![
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
        let mut state = state();
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.context("shell", "posix");
        state.input("cat saZZ other");
        for _ in 0..8 {
            state.edit_key("left", false);
        }
        state.refresh(false);
        let query = path_query(&state).unwrap();
        state.merge_candidates(
            query.candidates(vec![super::super::path_suggestions::Entry {
                name: "sample file".into(),
                directory: false,
            }]),
        );
        let edit = state.accept().unwrap();
        assert_eq!(state.text, "cat sample\\ file other");
        assert!(!edit.contains('\n'));
        state.refresh(false);
        state.input("x");
        assert!(state.accept().is_none());
    }
    #[test]
    fn fuzzy_and_middle_command_completion_replace_only_the_command_word() {
        let mut state = state();
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
        let mut state = state();
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
        let mut state = state();
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
        let mut state = state();
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
        assert!(state.is_editing());
    }
    #[test]
    fn only_shell_imported_commands_become_history_and_accept_never_submits() {
        let mut state = state();
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
        assert!(state.is_editing());
        assert_eq!(state.text, "git status");
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
        let mut state = state();
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
        assert!(!state.is_editing());
    }
    #[test]
    fn redraw_and_private_history_entries_do_not_create_candidates() {
        let mut state = state();
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.phase(PromptEvent::Input);
        assert!(state.is_editing());
        state.input("echo partial");
        state.phase(PromptEvent::Input);
        assert!(!state.is_editing());
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.input(" secret-token");
        state.submit();
        state.phase(PromptEvent::Execute);
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.trigger();
        assert!(state.candidates.is_empty());
    }
    #[test]
    fn shell_commands_merge_with_history_and_reset_per_shell() {
        let mut state = state();
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

mod cached_sessions {
    use super::*;
    fn paths(text: &str) -> Suggestions {
        let mut state = state();
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.context("shell", "posix");
        state.context("cwd", "/project");
        state.input(text);
        state.refresh(false);
        let query = path_query(&state).unwrap();
        state.merge_candidates(query.candidates(vec![
            super::super::path_suggestions::Entry {
                name: "alpha.txt".into(),
                directory: false,
            },
            super::super::path_suggestions::Entry {
                name: "apple.txt".into(),
                directory: false,
            },
        ]));
        state
    }
    #[test]
    fn deletion_can_restore_broader_cached_paths_without_requerying() {
        let mut state = paths("cat a");
        state.input("l");
        state.refresh_in_place(false, true);
        assert_eq!(
            state
                .candidates
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            ["alpha.txt"]
        );
        assert_eq!(state.candidates[0].replacement, 4..6);
        state.edit_key("backspace", false);
        state.refilter_cached();
        assert_eq!(state.candidates.len(), 2);
        assert!(
            state
                .candidates
                .iter()
                .all(|item| item.replacement == (4..5))
        );
        state.edit_key("backspace", false);
        state.edit_key("w", true);
        state.refilter_cached();
        assert!(state.candidates.is_empty());
    }
    #[test]
    fn cached_paths_do_not_cross_directory_quote_or_session_boundaries() {
        for suffix in ["/", "\""] {
            let mut state = paths("cat a");
            state.input(suffix);
            state.refresh_in_place(false, true);
            assert!(!state.has_current_candidates());
        }
        let mut state = paths("cat a");
        state.context("cwd", "/another");
        state.refilter_cached();
        assert!(state.candidates.is_empty());
    }
}

mod imported_history {
    use super::*;
    use terminal::HistoryEvent;
    fn batch(state: &mut Suggestions, id: u64, reset: bool, commands: &[&str]) {
        state.import_history(HistoryEvent::Session("active".into()));
        state.import_history(HistoryEvent::Begin {
            session: "active".into(),
            batch: id,
            reset,
        });
        for (index, command) in commands.iter().enumerate() {
            state.import_history(HistoryEvent::Entry {
                session: "active".into(),
                batch: id,
                index: index as u64,
                command: (*command).into(),
            });
        }
        state.import_history(HistoryEvent::End {
            session: "active".into(),
            batch: id,
            count: commands.len(),
        });
    }
    fn editing(state: &mut Suggestions, shell: &str, text: &str) {
        state.context("shell", shell);
        state.phase(PromptEvent::Prompt);
        state.phase(PromptEvent::Input);
        state.input(text);
        state.refresh(false);
    }
    #[test]
    fn history_case_matching_is_shell_specific_and_deletion_keeps_recency() {
        let mut state = state();
        batch(
            &mut state,
            1,
            true,
            &["Get-Item old", "Get-Item newest", "_helper real-history"],
        );
        editing(&mut state, "posix", "get-i");
        assert!(state.candidates.is_empty());
        editing(&mut state, "powershell", "get-it");
        assert_eq!(
            state
                .candidates
                .iter()
                .map(|item| item.insert_text.as_str())
                .collect::<Vec<_>>(),
            ["Get-Item newest", "Get-Item old"]
        );
        state.edit_key("backspace", false);
        state.refilter_cached();
        assert_eq!(state.candidates[0].insert_text, "Get-Item newest");
        editing(&mut state, "posix", "_he");
        assert_eq!(state.candidates[0].source, Source::History);
        assert_eq!(state.candidates[0].insert_text, "_helper real-history");
    }
    #[test]
    fn background_imports_preserve_open_selection_paths_and_closed_menu_state() {
        let mut state = state();
        batch(&mut state, 1, true, &["cat old-one", "cat old-two"]);
        editing(&mut state, "posix", "cat o");
        state.context("cwd", "/tmp");
        state.refresh(false);
        let mut path = Candidate::new(
            "other.txt".into(),
            state.text.len(),
            Kind::File,
            Source::Filesystem,
            1900,
        );
        path.replacement = 4..5;
        state.merge_candidates(vec![path]);
        state.selected = state
            .candidates
            .iter()
            .position(|item| item.insert_text == "cat old-one")
            .unwrap();
        let revision = state.revision;
        batch(&mut state, 2, false, &["cat old-three"]);
        assert_eq!(state.revision, revision); // In-flight path requests remain valid.
        assert_eq!(state.candidates[state.selected].insert_text, "cat old-one");
        assert!(
            state
                .candidates
                .iter()
                .any(|item| item.source == Source::Filesystem)
        );
        assert!(
            state
                .candidates
                .iter()
                .any(|item| item.insert_text == "cat old-three")
        );
        state.dismiss();
        batch(&mut state, 3, false, &["cat old-four"]);
        assert!(!state.has_current_candidates());
        state.trigger();
        assert!(
            state
                .candidates
                .iter()
                .any(|item| item.insert_text == "cat old-four")
        );
    }

    #[test]
    fn history_imports_keep_manually_opened_menus_when_automatic_is_disabled_or_input_is_empty() {
        for text in ["g", ""] {
            let mut state = state();
            state.settings.automatic = false;
            state.add_command("git");
            batch(&mut state, 1, true, &["git old"]);
            editing(&mut state, "posix", text);
            state.trigger();
            assert!(state.has_current_candidates());
            batch(&mut state, 2, false, &["git new"]);
            assert!(state.has_current_candidates());
            assert!(
                state
                    .candidates
                    .iter()
                    .any(|item| item.source == Source::Shell)
            );
            assert!(
                state
                    .candidates
                    .iter()
                    .any(|item| item.insert_text == "git new")
            );
        }
    }
    #[test]
    fn imported_history_keeps_recent_order_beyond_one_hundred_entries() {
        let mut state = state();
        let commands: Vec<_> = (0..200).map(|index| format!("echo {index:04}")).collect();
        batch(
            &mut state,
            1,
            true,
            &commands.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        editing(&mut state, "posix", "echo ");
        assert_eq!(state.candidates[150].insert_text, "echo 0049");
        state.edit_key("backspace", false);
        state.refilter_cached();
        assert_eq!(state.candidates[150].insert_text, "echo 0049");
    }
}
