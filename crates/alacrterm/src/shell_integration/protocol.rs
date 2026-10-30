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
    if let Some(history) = payload.strip_prefix(b"633;History;") {
        return decode_history(history).map(ShellMetadata::History);
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

// Hex is portable in Bash/Zsh/PowerShell and never contains OSC delimiters.
fn decode_history(payload: &[u8]) -> Option<terminal::HistoryEvent> {
    use terminal::HistoryEvent;
    let mut fields = std::str::from_utf8(payload).ok()?.split(';');
    let kind = fields.next()?;
    let session = fields.next()?;
    if session.is_empty()
        || session.len() > 80
        || !session
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == b'-')
    {
        return None;
    }
    if kind == "session" {
        return fields
            .next()
            .is_none()
            .then(|| HistoryEvent::Session(session.into()));
    }
    let batch = fields.next()?.parse().ok()?;
    let event = match kind {
        "begin" => HistoryEvent::Begin {
            session: session.into(),
            batch,
            reset: match fields.next()? {
                "snapshot" => true,
                "append" => false,
                _ => return None,
            },
        },
        "entry" => {
            let index = fields.next()?.parse().ok()?;
            let hex = fields.next()?;
            if hex.is_empty() || hex.len() > 8192 || !hex.len().is_multiple_of(2) {
                return None;
            }
            let bytes = hex
                .as_bytes()
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| {
                    let digit = |ch: u8| (ch as char).to_digit(16).map(|v| v as u8);
                    Some(digit(pair[0])? * 16 + digit(pair[1])?)
                })
                .collect::<Option<Vec<_>>>()?;
            let command = String::from_utf8(bytes).ok()?;
            if command.trim().is_empty() || command.chars().any(char::is_control) {
                return None;
            }
            HistoryEvent::Entry {
                session: session.into(),
                batch,
                index,
                command,
            }
        }
        "end" => {
            let count = fields.next()?.parse().ok()?;
            if count > 1000 {
                return None;
            }
            HistoryEvent::End {
                session: session.into(),
                batch,
                count,
            }
        }
        _ => return None,
    };
    fields.next().is_none().then_some(event)
}

#[cfg(test)]
mod history_tests {
    use super::*;
    use terminal::HistoryEvent;
    #[test]
    fn history_messages_validate_framing_and_decode_literal_utf8() {
        assert_eq!(
            decode(b"633;History;session;local-123"),
            Some(ShellMetadata::History(HistoryEvent::Session(
                "local-123".into()
            )))
        );
        assert_eq!(
            decode(b"633;History;entry;local-123;1;42;6563686f203b20e4b8ad"),
            Some(ShellMetadata::History(HistoryEvent::Entry {
                session: "local-123".into(),
                batch: 1,
                index: 42,
                command: "echo ; 中".into()
            }))
        );
        for invalid in [
            "633;History;session;bad session",
            "633;History;session;s;extra",
            "633;History;begin;s;1;invalid",
            "633;History;entry;s;1;2;zz",
            "633;History;entry;s;1;2;ff",
            "633;History;entry;s;1;2;0a",
            "633;History;entry;s;1;2;1b",
            "633;History;entry;s;1;2;20",
            "633;History;entry;s;1;2;616",
            "633;History;end;s;1;1001",
            "633;History;end;s;1;0;extra",
        ] {
            assert_eq!(decode(invalid.as_bytes()), None, "{invalid}");
        }
        let oversized = format!("633;History;entry;s;1;2;{}", "61".repeat(4097));
        assert_eq!(decode(oversized.as_bytes()), None);
    }
}
