use gpui::*;
use gpui::prelude::*;
use std::sync::Arc;
use std::time::Duration;
use terminal::Terminal;
use alacritty_terminal::term::Config;

pub struct TerminalView {
    terminal: Arc<Terminal>,
    focus_handle: FocusHandle,
    cursor_visible: bool,
    last_size: Option<(usize, usize)>, // (cols, lines)
    scroll_offset: usize, // 滚动偏移量，0 表示最底部（当前内容）
}

impl TerminalView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let config = Config::default();
        let terminal = Arc::new(Terminal::new(&config));

        // 启动定时刷新机制，每16ms检查一次（60fps，仅在有更新时渲染）
        let terminal_clone = terminal.clone();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(16)).await;

                // 仅在终端有更新时才触发渲染
                if terminal_clone.needs_render() {
                    let _ = this.update(cx, |_, cx| {
                        cx.notify();
                    });
                }
            }
        })
        .detach();

        // 启动光标闪烁定时器，每500ms切换一次
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(500)).await;
                let _ = this.update(cx, |view, cx| {
                    view.cursor_visible = !view.cursor_visible;
                    cx.notify();
                });
            }
        })
        .detach();

        Self {
            terminal,
            focus_handle: cx.focus_handle(),
            cursor_visible: true,
            last_size: None,
            scroll_offset: 0,
        }
    }

    fn handle_input(&self, input: &str) {
        self.terminal.write(input.as_bytes());
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 获取窗口尺寸并计算终端大小
        let window_bounds = window.bounds();
        let char_width = 8.4;
        let line_height = 16.8;

        // 将 Pixels 转换为 f32
        let available_width: f32 = window_bounds.size.width.into();
        let available_height: f32 = window_bounds.size.height.into();

        // 减去标题栏（约32px）和 padding（16px）
        let available_width = available_width - 16.0;
        let available_height = available_height - 48.0;

        // 计算可容纳的列数和行数
        let cols = ((available_width / char_width).floor().max(1.0) as usize).max(80);
        let lines = ((available_height / line_height).floor().max(1.0) as usize).max(24);

        // 检查尺寸是否变化，如果变化则调整终端大小
        if self.last_size != Some((cols, lines)) {
            self.last_size = Some((cols, lines));
            self.terminal.resize(cols, lines);
        }

        // 使用滚动偏移量获取内容
        let content = self.terminal.get_content_with_scroll(self.scroll_offset);
        let (cursor_col, cursor_line) = self.terminal.cursor_position();
        let focus_handle = self.focus_handle.clone();
        let cursor_visible = self.cursor_visible;
        let history_size = self.terminal.history_size();

        div()
            .id("terminal")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(rgb(0x1a1b26))
            .text_color(rgb(0xc0caf5))
            .p_2()
            .on_mouse_down(MouseButton::Left, cx.listener(move |_this, _event, window, cx| {
                window.focus(&focus_handle);
                cx.stop_propagation();
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, _cx| {
                let keystroke = &event.keystroke;

                // 任何键盘输入都重置滚动到底部
                if this.scroll_offset > 0 {
                    this.scroll_offset = 0;
                }

                // 处理特殊键
                let input = if keystroke.key == "enter" {
                    "\r".to_string()
                } else if keystroke.key == "backspace" {
                    "\x7f".to_string()
                } else if keystroke.key == "tab" {
                    "\t".to_string()
                } else if keystroke.key == "space" {
                    " ".to_string()
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
                    .flex_row()
                    .size_full()
                    .child(
                        // 终端内容区域
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .font_family("monospace")
                            .text_size(px(14.0))
                            .line_height(relative(1.2))
                            .on_scroll_wheel(cx.listener(move |this, event: &ScrollWheelEvent, _window, cx| {
                                use gpui::ScrollDelta;

                                // 计算滚动行数
                                let scroll_lines = match &event.delta {
                                    ScrollDelta::Pixels(point) => {
                                        let delta_y: f32 = point.y.into();
                                        (delta_y / 20.0).round() as i32
                                    }
                                    ScrollDelta::Lines(point) => {
                                        let delta_y: f32 = point.y.into();
                                        (delta_y * 3.0).round() as i32
                                    }
                                };

                                // 更新滚动偏移量
                                let history_size = this.terminal.history_size();
                                if history_size > 0 && scroll_lines != 0 {
                                    let new_offset = (this.scroll_offset as i32 + scroll_lines)
                                        .max(0)
                                        .min(history_size as i32) as usize;

                                    if new_offset != this.scroll_offset {
                                        this.scroll_offset = new_offset;
                                        cx.notify();
                                    }
                                }

                                cx.stop_propagation();
                            }))
                            .children(content.iter().enumerate().map(|(line_idx, line_text)| {
                                // 只在滚动到底部时显示光标
                                let is_cursor_line = self.scroll_offset == 0 && line_idx == cursor_line;

                                if is_cursor_line && cursor_col < line_text.len() {
                                    // 光标所在行：分成三部分渲染（光标前、光标、光标后）
                                    let chars: Vec<char> = line_text.chars().collect();
                                    let before = chars[..cursor_col].iter().collect::<String>();
                                    let cursor_char = chars[cursor_col].to_string();
                                    let after = chars[cursor_col + 1..].iter().collect::<String>();

                                    div()
                                        .flex()
                                        .child(before)
                                        .child(
                                            div()
                                                .flex_shrink_0()
                                                .w(px(8.4))
                                                .bg(if cursor_visible { rgb(0x7aa2f7) } else { rgb(0x1a1b26) })
                                                .child(cursor_char)
                                        )
                                        .child(after)
                                } else {
                                    // 普通行：直接渲染整行文本（性能优化）
                                    div().child(line_text.clone())
                                }
                            }))
                    )
                    .child(
                        // 滚动条区域 - 始终显示
                        div()
                            .relative()
                            .w(px(12.0))
                            .h_full()
                            .bg(rgb(0x24283b))
                            .border_l_1()
                            .border_color(rgb(0x1a1b26))
                            .when(history_size > 0, |container: Div| {
                                let total_content = history_size + lines;
                                let visible_ratio = lines as f32 / total_content as f32;
                                let scroll_ratio = self.scroll_offset as f32 / total_content as f32;

                                // 计算滚动条滑块的高度和位置
                                let scrollbar_height = (available_height * visible_ratio).max(30.0);
                                let scrollbar_top = available_height * (1.0 - scroll_ratio - visible_ratio);

                                container
                                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                                        // 点击滚动条轨道，跳转到对应位置
                                        let click_y: f32 = event.position.y.into();
                                        let track_height = available_height;

                                        // 计算点击位置对应的滚动比例（从顶部开始，0=顶部历史，1=底部当前）
                                        let click_ratio = (click_y / track_height).clamp(0.0, 1.0);

                                        // 转换为滚动偏移量（从底部开始计数）
                                        let new_scroll_ratio = 1.0 - click_ratio;
                                        let new_offset = (new_scroll_ratio * total_content as f32) as usize;
                                        this.scroll_offset = new_offset.min(history_size);
                                        cx.notify();
                                    }))
                                    .child(
                                        div()
                                            .absolute()
                                            .top(px(scrollbar_top))
                                            .left(px(2.0))
                                            .w(px(8.0))
                                            .h(px(scrollbar_height))
                                            .bg(rgb(0x7aa2f7))
                                            .rounded(px(4.0))
                                            .shadow_sm()
                                    )
                            })
                    )
            )
    }
}
