//! Real-shell regression tests. Optionally include a separately built old Bash.
use std::{
    collections::HashMap,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

fn bash_programs() -> Vec<PathBuf> {
    let mut programs = vec![PathBuf::from("/bin/bash")];
    if let Some(program) = std::env::var_os("ALACRTERM_TEST_BASH") {
        let program = PathBuf::from(program);
        assert!(
            program.is_file(),
            "test Bash binary missing: {}",
            program.display()
        );
        if !programs.contains(&program) {
            programs.push(program);
        }
    }
    programs
}

fn run_bash(program: &Path, rc: &str, input: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join(".bashrc"), rc).unwrap();
    let mut env = HashMap::from([("HOME".into(), home.path().to_string_lossy().into_owned())]);
    let (args, _files) = super::local::prepare(program.to_str().unwrap(), &[], &mut env)
        .unwrap()
        .unwrap();
    let mut child = Command::new(program)
        .args(args)
        .envs(env)
        .env_remove("BASH_ENV")
        .env_remove("ENV")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}: {}",
        program.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.stdout, output.stderr)
}

#[test]
fn bash_hooks_preserve_status_and_do_not_execute_on_blank_lines() {
    for program in bash_programs() {
        for hooks in [
            "PROMPT_COMMAND='printf \"USER_STATUS=%s\\n\" \"$?\"'",
            "PROMPT_COMMAND=('printf \"USER_STATUS=%s\\n\" \"$?\"' 'printf \"USER_SECOND\\n\"')",
        ] {
            // The wrapper sources the integration again, exercising idempotence.
            let rc = format!("PS1='test$ '\n{hooks}\n. \"$ALACRTERM_INTEGRATION_SCRIPT\"\n");
            let (stdout, stderr) = run_bash(
                &program,
                &rc,
                b"false\n\nprintf 'USER_RESULT=%s\\n' \"$?\"\nexit\n",
            );
            let mut all = stdout.clone();
            all.extend(stderr);
            let text = String::from_utf8_lossy(&all);
            assert_eq!(
                text.matches("\x1b]633;C\x07").count(),
                3,
                "{}: {text}",
                program.display()
            );
            assert!(text.contains("\x1b]633;D;1\x07"));
            assert_eq!(text.matches("\x1b]633;AlacrtermShell;posix\x07").count(), 1);
            let output = String::from_utf8_lossy(&stdout);
            assert!(output.contains("USER_STATUS=1\n"));
            assert!(output.contains("USER_RESULT=1\n"));
            if hooks.contains("USER_SECOND") {
                assert!(output.contains("USER_SECOND\n"));
            }
        }
    }
}

#[test]
fn command_inventory_is_unique_and_only_retransmitted_when_names_change() {
    for program in bash_programs() {
        let (stdout, _) = run_bash(
            &program,
            "mkdir -p \"$HOME/bin\"\nPATH=\"$HOME/bin:$PATH\"\n",
            b"alacrterm_refresh_completions\nalias freshalias=:\nalacrterm_refresh_completions\nunalias freshalias\nprintf '' > \"$HOME/bin/periodic-tool\"; chmod +x \"$HOME/bin/periodic-tool\"\nSECONDS=$((SECONDS + 31))\nalacrterm_refresh_completions\nexit\n",
        );
        let text = String::from_utf8_lossy(&stdout);
        // Initial inventory, alias addition/removal, and periodic PATH rescan.
        assert_eq!(
            text.matches("\x1b]633;CommandReset\x07").count(),
            4,
            "{}",
            program.display()
        );
        assert!(text.contains("\x1b]633;Command;freshalias\x07"));
        assert!(text.contains("\x1b]633;Command;periodic-tool\x07"));
        for inventory in text.split("\x1b]633;CommandReset\x07").skip(1) {
            let names: Vec<_> = inventory
                .split("\x1b]633;Command;")
                .skip(1)
                .map(|record| record.split('\x07').next().unwrap())
                .collect();
            let unique: std::collections::HashSet<_> = names.iter().collect();
            assert_eq!(names.len(), unique.len(), "duplicate inventory names");
        }
    }
}

#[test]
fn existing_debug_traps_are_preserved() {
    for program in bash_programs() {
        let (stdout, stderr) = run_bash(
            &program,
            "trap 'printf USER_DEBUG\\n' DEBUG\n",
            b"trap -p DEBUG\nprintf 'WORKED\\n'\nexit\n",
        );
        let output = String::from_utf8_lossy(&stdout);
        assert!(output.contains("USER_DEBUG"));
        assert!(output.contains("trap -- 'printf USER_DEBUG"));
        assert!(!output.contains("trap -- '__alacrterm_preexec'"));
        assert!(output.contains("WORKED\n"));
        assert!(!String::from_utf8_lossy(&stderr).contains("syntax error"));
    }
}

#[test]
fn posix_shell_parses_and_reports_prompt_context_without_fake_execution_events() {
    let shell = if Path::new("/bin/dash").is_file() {
        "/bin/dash"
    } else {
        "/bin/sh"
    };
    let home = tempfile::tempdir().unwrap();
    let script = home.path().join("integration.sh");
    std::fs::write(
        &script,
        include_str!("../../../../scripts/shell_integration/bash.sh"),
    )
    .unwrap();
    assert!(
        Command::new(shell)
            .arg("-n")
            .arg(&script)
            .status()
            .unwrap()
            .success()
    );
    let mut child = Command::new(shell)
        .arg("-i")
        .env("HOME", home.path())
        .env_remove("BASH_ENV")
        .env_remove("ENV")
        .env_remove("BASH_VERSION")
        .env("PWD", home.path().canonicalize().unwrap())
        .current_dir(home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            format!(
                ". '{}'\nfalse\nprintf 'POSIX_WORKED\\n'\nexit\n",
                script.display()
            )
            .as_bytes(),
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let mut all = output.stdout.clone();
    all.extend(output.stderr);
    let text = String::from_utf8_lossy(&all);
    assert!(String::from_utf8_lossy(&output.stdout).contains("POSIX_WORKED\n"));
    assert!(text.contains("\x1b]633;AlacrtermShell;posix\x07"));
    assert!(text.contains(&format!(
        "\x1b]633;AlacrtermCwd;{}\x07",
        home.path().canonicalize().unwrap().display()
    )));
    assert!(text.contains("\x1b]633;B\x07"));
    assert!(!text.contains("syntax error"));
    if shell.ends_with("dash") {
        assert!(!text.contains("\x1b]633;C\x07"));
    }
}

fn history_events(bytes: &[u8]) -> Vec<terminal::HistoryEvent> {
    bytes
        .split(|byte| *byte == 7)
        .filter_map(|part| {
            let start = part.windows(2).rposition(|bytes| bytes == b"\x1b]")?;
            match super::protocol::decode(&part[start + 2..])? {
                terminal::ShellMetadata::History(event) => Some(event),
                _ => None,
            }
        })
        .collect()
}

#[test]
fn bash_imports_real_history_and_syncs_only_accepted_single_line_entries() {
    use terminal::HistoryEvent;
    for program in bash_programs() {
        let (stdout, stderr) = run_bash(&program,
            "HISTCONTROL=ignorespace\nHISTIGNORE='*IGNORED_BY_POLICY*'\nhistory -c\nhistory -s 'echo imported-old'\nhistory -s $'echo multiline-first\\necho multiline-second'\n",
            b"sleep .15\n echo PRIVATE_IGNORED_SPACE\necho IGNORED_BY_POLICY\necho incremental-new\nsleep .15\nhistory -c\nsleep .15\nexit\n");
        let events = history_events(&stdout);
        let commands: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                HistoryEvent::Entry { command, .. } => Some(command.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            commands.contains(&"echo imported-old"),
            "{events:?}\n{}",
            String::from_utf8_lossy(&stderr)
        );
        assert!(commands.contains(&"echo incremental-new"), "{events:?}");
        assert!(
            !commands
                .iter()
                .any(|command| command.contains("PRIVATE_IGNORED_SPACE")
                    || command.contains("IGNORED_BY_POLICY")
                    || command.contains("multiline-"))
        );
        assert!(
            events
                .iter()
                .filter(|event| matches!(event, HistoryEvent::Begin { reset: true, .. }))
                .count()
                >= 2,
            "history -c must reset cache: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, HistoryEvent::Begin { reset: false, .. }))
        );
    }
}

#[test]
fn disabling_history_does_not_read_or_transmit_commands() {
    for program in bash_programs() {
        let (stdout, _) = run_bash(
            &program,
            "ALACRTERM_HISTORY_ENABLED=0\nhistory -s 'echo private-old'\n",
            b"echo private-new\nsleep .1\nexit\n",
        );
        assert!(!history_events(&stdout).iter().any(|event| matches!(
            event,
            terminal::HistoryEvent::Entry { .. } | terminal::HistoryEvent::Begin { .. }
        )));
    }
}

#[test]
fn zsh_imports_loaded_history_and_literal_utf8_without_multiline_fragments() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join(".zshrc"),
        "HISTSIZE=2000\nprint -s 'echo imported-zsh-中文'\nprint -s $'echo multiline-first\\necho multiline-second'\n").unwrap();
    let mut env = HashMap::from([("HOME".into(), home.path().to_string_lossy().into_owned())]);
    let (args, _files) = super::local::prepare("zsh", &[], &mut env)
        .unwrap()
        .unwrap();
    let mut child = Command::new("zsh")
        .args(args)
        .envs(env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"sleep .15\necho incremental-zsh\nsleep .15\nexit\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let events = history_events(&output.stdout);
    let commands: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            terminal::HistoryEvent::Entry { command, .. } => Some(command.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        commands.contains(&"echo imported-zsh-中文"),
        "{events:?}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(commands.contains(&"echo incremental-zsh"));
    assert!(
        !commands
            .iter()
            .any(|command| command.contains("multiline-"))
    );
}

#[test]
fn commands_executed_during_initial_import_are_queued_without_an_extra_prompt() {
    for program in bash_programs() {
        let rc = r#"
history -c
history -s 'echo INITIAL_HISTORY'
. "$ALACRTERM_INTEGRATION_SCRIPT"
eval "$(declare -f __alacrterm_history_collect | sed '1s/__alacrterm_history_collect/__alacrterm_saved_collect/')"
__alacrterm_history_collect() { sleep .1; __alacrterm_saved_collect "$@"; }
"#;
        let (stdout, _) = run_bash(
            &program,
            rc,
            b"echo QUEUED_FIRST\necho QUEUED_SECOND\nsleep .5\nexit\n",
        );
        let events = history_events(&stdout);
        for expected in [
            "echo INITIAL_HISTORY",
            "echo QUEUED_FIRST",
            "echo QUEUED_SECOND",
        ] {
            assert!(events.iter().any(|event| matches!(event, terminal::HistoryEvent::Entry { command, .. } if command == expected)), "{expected}: {events:?}");
        }
        let mut active = None;
        for event in events {
            match event {
                terminal::HistoryEvent::Begin { batch, .. } => {
                    assert!(active.replace(batch).is_none(), "interleaved batches");
                }
                terminal::HistoryEvent::End { batch, .. } => {
                    assert_eq!(active.take(), Some(batch));
                }
                _ => {}
            }
        }
        assert!(active.is_none());
    }
}

#[test]
fn history_workers_preserve_the_users_last_background_pid() {
    for program in bash_programs() {
        let (stdout, stderr) = run_bash(
            &program,
            "history -c\n",
            b"sleep 5 &\nprintf 'USER_PID=%s JOB_PID=%s\\n' $! \"$(jobs -p)\"\nkill $!\nexit\n",
        );
        let text = String::from_utf8_lossy(&stdout);
        let line = text
            .lines()
            .find(|line| line.contains("USER_PID="))
            .unwrap_or_else(|| {
                panic!(
                    "{}: {text}\n{}",
                    program.display(),
                    String::from_utf8_lossy(&stderr)
                )
            });
        let values = line.split("USER_PID=").nth(1).unwrap();
        let (pid, job) = values.split_once(" JOB_PID=").unwrap();
        let job: String = job.chars().take_while(char::is_ascii_digit).collect();
        assert!(!job.is_empty());
        assert_eq!(pid, job, "{text}");
    }
}

#[test]
fn returning_from_a_nested_bash_restores_parent_history() {
    for program in bash_programs() {
        let (stdout, stderr) = run_bash(&program, "history -c\nhistory -s 'echo parent-existing'\n",
            b"\"$BASH\" --noprofile --norc -ic '. \"$ALACRTERM_INTEGRATION_SCRIPT\"; history -s \"echo nested-only\"; __alacrterm_history_sync; sleep .15'\necho parent-after\nsleep .3\nexit\n");
        let events = history_events(&stdout);
        let mut sessions: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                terminal::HistoryEvent::Session(id) => Some(id),
                _ => None,
            })
            .collect();
        sessions.dedup();
        assert!(
            sessions.len() >= 3,
            "{events:?}\n{}",
            String::from_utf8_lossy(&stderr)
        );
        assert_eq!(sessions.first(), sessions.last());
        let mut model = terminal_view::SuggestionModel::default();
        model.service = Some(std::sync::Arc::new(
            crate::completion::CompletionEngine::default(),
        ));
        for event in events {
            model.import_history(event);
        }
        model.phase(terminal::PromptEvent::Prompt);
        model.phase(terminal::PromptEvent::Input);
        model.input("echo ");
        model.refresh(false);
        assert!(
            model
                .candidates
                .iter()
                .any(|item| item.insert_text == "echo parent-existing")
        );
        assert!(
            model
                .candidates
                .iter()
                .any(|item| item.insert_text == "echo parent-after")
        );
        assert!(
            !model
                .candidates
                .iter()
                .any(|item| item.insert_text == "echo nested-only")
        );
    }
}
