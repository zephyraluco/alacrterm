//! Bounded OSC 133/633 shell lifecycle scanner, shared by PTY and SSH output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptEvent {
    Prompt,
    Input,
    Execute,
    Finished,
}
#[derive(Default)]
pub(crate) struct PromptScanner {
    state: u8,
    payload: Vec<u8>,
    commands: Vec<String>,
    context: Vec<(&'static str, String)>,
}
impl PromptScanner {
    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Vec<PromptEvent> {
        let mut events = Vec::new();
        for &byte in bytes {
            match (self.state, byte) {
                (0, 27) => self.state = 1,
                (1, b']') => {
                    self.payload.clear();
                    self.state = 2;
                }
                (1, _) => self.state = 0,
                (2, 7) | (3, b'\\') => {
                    if let Some(cwd) = self
                        .payload
                        .strip_prefix(b"633;AlacrtermCwd;")
                        .and_then(|bytes| std::str::from_utf8(bytes).ok())
                    {
                        self.context.push((
                            "cwd",
                            if cwd.chars().any(char::is_control) {
                                String::new()
                            } else {
                                cwd.into()
                            },
                        ));
                    }
                    if let Some(shell) = self
                        .payload
                        .strip_prefix(b"633;AlacrtermShell;")
                        .and_then(|bytes| std::str::from_utf8(bytes).ok())
                    {
                        if matches!(shell, "posix" | "powershell") {
                            self.context.push(("shell", shell.into()));
                        }
                    }
                    if self.payload == b"633;CommandReset" {
                        self.commands.push(String::new());
                    } else if let Some(name) = self
                        .payload
                        .strip_prefix(b"633;Command;")
                        .and_then(|name| std::str::from_utf8(name).ok())
                    {
                        if !name.is_empty()
                            && name.len() <= 256
                            && name
                                .chars()
                                .all(|ch| ch.is_ascii_alphanumeric() || "_.:+-".contains(ch))
                        {
                            self.commands.push(name.to_string());
                        }
                    }
                    if let Some(event) = self.event() {
                        events.push(event);
                    }
                    self.payload.clear();
                    self.state = 0;
                }
                (2, 27) => self.state = 3,
                (2, _) if self.payload.len() < 4096 => self.payload.push(byte),
                (2, _) => {
                    self.payload.clear();
                    self.state = 4;
                }
                (3, _) => {
                    self.payload.clear();
                    self.state = 0;
                }
                (4, 7) => self.state = 0,
                (4, 27) => self.state = 5,
                (5, b'\\') => self.state = 0,
                (5, _) => self.state = 4,
                _ => {}
            }
        }
        events
    }
    pub(crate) fn context(&mut self) -> Vec<(&'static str, String)> {
        std::mem::take(&mut self.context)
    }

    pub(crate) fn commands(&mut self) -> Vec<String> {
        std::mem::take(&mut self.commands)
    }

    fn event(&self) -> Option<PromptEvent> {
        let mut fields = self.payload.split(|byte| *byte == b';');
        if !matches!(fields.next()?, b"633" | b"133") {
            return None;
        }
        match fields.next()? {
            b"A" => Some(PromptEvent::Prompt),
            b"B" => Some(PromptEvent::Input),
            b"C" => Some(PromptEvent::Execute),
            b"D" => Some(PromptEvent::Finished),
            _ => None,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragmented_markers_ignore_other_osc_and_recover_from_oversize() {
        let mut scanner = PromptScanner::default();
        assert!(scanner.feed(b"\x1b]633;").is_empty());
        assert_eq!(
            scanner.feed(b"B\x1b\\\x1b]2;title\x07\x1b]133;C\x07"),
            [PromptEvent::Input, PromptEvent::Execute]
        );
        let mut oversized = b"\x1b]633;".to_vec();
        oversized.extend(vec![b'A'; 5000]);
        oversized.extend_from_slice(b"\x07\x1b]633;D;0\x07");
        assert_eq!(scanner.feed(&oversized), [PromptEvent::Finished]);
    }
    #[test]
    fn working_directory_context_preserves_literal_paths() {
        let mut scanner = PromptScanner::default();
        scanner.feed(b"\x1b]633;AlacrtermCwd;/remote/My ");
        scanner.feed(b"Files;a\x07\x1b]633;AlacrtermShell;posix\x07");
        assert_eq!(
            scanner.context(),
            [
                ("cwd", "/remote/My Files;a".into()),
                ("shell", "posix".into())
            ]
        );
        scanner.feed(b"\x1b]633;AlacrtermCwd;/bad\npath\x07\x1b]633;AlacrtermShell;unknown\x07");
        assert_eq!(scanner.context(), [("cwd", String::new())]);
    }
    #[test]
    fn command_inventory_is_bounded_to_literal_command_names() {
        let mut scanner = PromptScanner::default();
        assert!(
            scanner
                .feed(b"\x1b]633;CommandReset\x07\x1b]633;Command;remote-")
                .is_empty()
        );
        scanner.feed(b"tool\x07\x1b]633;Command;bad;command\x07\x1b]633;Command;bad name\x07");
        assert_eq!(scanner.commands(), ["", "remote-tool"]);
        assert!(scanner.commands().is_empty());
    }
}
