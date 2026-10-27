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
