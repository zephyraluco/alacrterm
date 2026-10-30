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
    history: Vec<crate::HistoryEvent>,
    context: Vec<(&'static str, String)>,
}
impl PromptScanner {
    pub(crate) fn feed(
        &mut self,
        bytes: &[u8],
        decoder: Option<crate::OscDecoder>,
    ) -> Vec<PromptEvent> {
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
                    if let Some(metadata) = decoder.and_then(|decode| decode(&self.payload)) {
                        match metadata {
                            crate::ShellMetadata::Command(command) => self.commands.push(command),
                            crate::ShellMetadata::History(history) => self.history.push(history),
                            crate::ShellMetadata::Context(key, value) => {
                                self.context.push((key, value))
                            }
                        }
                    }
                    if let Some(event) = self.event() {
                        events.push(event);
                    }
                    self.payload.clear();
                    self.state = 0;
                }
                (2, 27) => self.state = 3,
                (2, _) if self.payload.len() < 12288 => self.payload.push(byte),
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

    pub(crate) fn history(&mut self) -> Vec<crate::HistoryEvent> {
        std::mem::take(&mut self.history)
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
    fn host_decoder_receives_complete_extensions_across_chunks() {
        fn decode(payload: &[u8]) -> Option<crate::ShellMetadata> {
            let value = std::str::from_utf8(payload.strip_prefix(b"999;cwd;")?).ok()?;
            Some(crate::ShellMetadata::Context("cwd", value.into()))
        }
        let mut scanner = PromptScanner::default();
        scanner.feed(b"\x1b]999;cwd;/some/", Some(decode));
        assert!(scanner.context().is_empty());
        assert_eq!(
            scanner.feed(b"path\x1b\\\x1b]633;B\x07", Some(decode)),
            [PromptEvent::Input]
        );
        assert_eq!(scanner.context(), [("cwd", "/some/path".into())]);
        scanner.feed(b"\x1b]999;cwd;/ignored\x07", None);
        assert!(scanner.context().is_empty());
    }
    #[test]
    fn fragmented_markers_ignore_other_osc_and_recover_from_oversize() {
        let mut scanner = PromptScanner::default();
        assert!(scanner.feed(b"\x1b]633;", None).is_empty());
        assert_eq!(
            scanner.feed(b"B\x1b\\\x1b]2;title\x07\x1b]133;C\x07", None),
            [PromptEvent::Input, PromptEvent::Execute]
        );
        let mut oversized = b"\x1b]633;".to_vec();
        oversized.extend(vec![b'A'; 13000]);
        oversized.extend_from_slice(b"\x07\x1b]633;D;0\x07");
        assert_eq!(scanner.feed(&oversized, None), [PromptEvent::Finished]);
    }
}
