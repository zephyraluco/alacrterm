//! Decode Alacrterm-specific OSC extensions; standard lifecycle stays in terminal.
use terminal::ShellMetadata;

pub(super) fn decode(payload: &[u8]) -> Option<ShellMetadata> {
    if let Some(cwd) = payload
        .strip_prefix(b"633;AlacrtermCwd;")
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
    {
        return Some(ShellMetadata::Context(
            "cwd",
            if cwd.chars().any(char::is_control) {
                String::new()
            } else {
                cwd.into()
            },
        ));
    }
    if let Some(shell) = payload
        .strip_prefix(b"633;AlacrtermShell;")
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
    {
        return matches!(shell, "posix" | "powershell")
            .then(|| ShellMetadata::Context("shell", shell.into()));
    }
    if payload == b"633;CommandReset" {
        return Some(ShellMetadata::Command(String::new()));
    }
    let name = std::str::from_utf8(payload.strip_prefix(b"633;Command;")?).ok()?;
    (!name.is_empty()
        && name.len() <= 256
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "_.:+-".contains(ch)))
    .then(|| ShellMetadata::Command(name.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extensions_validate_names_and_preserve_literal_paths() {
        assert_eq!(
            decode(b"633;AlacrtermCwd;/remote/My Files;a"),
            Some(ShellMetadata::Context("cwd", "/remote/My Files;a".into()))
        );
        assert_eq!(
            decode(b"633;AlacrtermCwd;/bad\npath"),
            Some(ShellMetadata::Context("cwd", String::new()))
        );
        assert_eq!(
            decode(b"633;AlacrtermShell;posix"),
            Some(ShellMetadata::Context("shell", "posix".into()))
        );
        assert_eq!(
            decode(b"633;CommandReset"),
            Some(ShellMetadata::Command(String::new()))
        );
        assert_eq!(
            decode(b"633;Command;remote-tool"),
            Some(ShellMetadata::Command("remote-tool".into()))
        );
        for payload in [
            b"633;Command;bad;name".as_slice(),
            b"633;Command;bad name",
            b"633;AlacrtermShell;unknown",
            b"633;B",
        ] {
            assert_eq!(decode(payload), None);
        }
    }
}
