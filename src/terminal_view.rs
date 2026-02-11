use gpui::*;
use std::sync::Arc;
use terminal::Terminal;
use alacritty_terminal::term::Config;

pub struct TerminalView {
    terminal: Arc<Terminal>,
    focus_handle: FocusHandle,
}

impl TerminalView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let config = Config::default();
        let terminal = Arc::new(Terminal::new(&config));

        Self {
            terminal,
            focus_handle: cx.focus_handle(),
        }
    }

    fn handle_input(&self, input: &str) {
        self.terminal.write(input.as_bytes());
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = self.terminal.get_content();
        let (cursor_col, cursor_line) = self.terminal.cursor_position();

        div()
            .id("terminal")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(rgb(0x1a1b26))
            .text_color(rgb(0xc0caf5))
            .p_2()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, _cx| {
                let keystroke = &event.keystroke;

                // 处理特殊键
                let input = if keystroke.key == "enter" {
                    "\r".to_string()
                } else if keystroke.key == "backspace" {
                    "\x7f".to_string()
                } else if keystroke.key == "tab" {
                    "\t".to_string()
                } else if keystroke.key == "escape" {
                    "\x1b".to_string()
                } else if keystroke.key == "up" {
                    "\x1b[A".to_string()
                } else if keystroke.key == "down" {
                    "\x1b[B".to_string()
                } else if keystroke.key == "right" {
                    "\x1b[C".to_string()
                } else if keystroke.key == "left" {
                    "\x1b[D".to_string()
                } else if keystroke.key == "home" {
                    "\x1b[H".to_string()
                } else if keystroke.key == "end" {
                    "\x1b[F".to_string()
                } else if keystroke.key == "pageup" {
                    "\x1b[5~".to_string()
                } else if keystroke.key == "pagedown" {
                    "\x1b[6~".to_string()
                } else if keystroke.key == "delete" {
                    "\x1b[3~".to_string()
                } else if keystroke.modifiers.control && keystroke.key.len() == 1 {
                    // 处理 Ctrl 组合键
                    let ch = keystroke.key.chars().next().unwrap();
                    if ch.is_ascii_alphabetic() {
                        let ctrl_char = (ch.to_ascii_lowercase() as u8 - b'a' + 1) as char;
                        ctrl_char.to_string()
                    } else {
                        keystroke.key.clone()
                    }
                } else {
                    keystroke.key.clone()
                };

                this.handle_input(&input);
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .font_family("monospace")
                    .text_size(px(14.0))
                    .line_height(relative(1.2))
                    .children(content.iter().enumerate().map(|(line_idx, line_text)| {
                        div()
                            .flex()
                            .child(
                                div()
                                    .flex()
                                    .children(line_text.chars().enumerate().map(move |(col_idx, ch)| {
                                        let is_cursor = line_idx == cursor_line && col_idx == cursor_col;
                                        div()
                                            .flex_shrink_0()
                                            .w(px(8.4))
                                            .h(px(16.8))
                                            .bg(if is_cursor { rgb(0x7aa2f7) } else { rgb(0x1a1b26) })
                                            .child(ch.to_string())
                                    }))
                            )
                    }))
            )
    }
}
