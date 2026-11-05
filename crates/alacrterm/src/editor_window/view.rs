//! Editor window layout and presentation.
use super::*;

impl EditorWindow {
    /// 工具条:图标按钮,按「文件 / 撤改 / 剪贴 / 查找 / 编辑器」分组,组间一条竖线。
    ///
    /// ⚠️ 除了「保存 / 关闭 / 跳转行 / 自动换行」,其余按钮**不是本窗口实现
    /// 的**:它们只把 gpui-base 编辑器那几个 action(`gpui_kit::component::input`)派发给
    /// **当前聚焦的元素**,由编辑器的动作处理器接下(见 [`action_button`] 与 [`EditorState`]
    /// 渲染里那串 `on_action`)。派发要落到编辑器身上,前提是点按钮不换焦点 —— gpui-kit 的
    /// `Button` 在 `on_mouse_down` 里 `prevent_default()`,正是为此。
    ///
    /// 窗口级那几枚走 `cx.listener` 直接调方法,不绕 action:按钮就在本窗口里,拿得到
    /// `Context<EditorWindow>`,直说比「派发给当前焦点」少一层不确定性(焦点不在编辑器上时
    /// 派发也会到,但那是运气)。
    ///
    /// Editing actions appear after the active document has loaded successfully.
    pub(super) fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let text = match self.active().map(|tab| &tab.body) {
            Some(Body::Text(text)) => Some(text),
            _ => None,
        };

        let mut bar = h_flex()
            .h(TOOLBAR_HEIGHT)
            .w_full()
            .flex_none()
            .px_2()
            .gap_0p5()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border);

        if let Some(text) = text {
            bar = bar
                .child(
                    Button::new("preview-save")
                        .ghost()
                        .xsmall()
                        .icon(crate::assets::ui_icon(IconName::Save))
                        .tooltip("保存到远端 (Ctrl+S)")
                        .disabled(text.is_saving() || !text.is_dirty())
                        .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                )
                .child(
                    Button::new("preview-close")
                        .ghost()
                        .xsmall()
                        .icon(crate::assets::ui_icon(IconName::WindowClose))
                        .tooltip("关闭标签 (Ctrl+W)")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.request_close_self(window, cx)),
                        ),
                )
                .child(toolbar_separator(cx))
                .child(action_button(
                    "preview-undo",
                    IconName::Undo2,
                    "撤销 (Ctrl+Z)",
                    Undo,
                ))
                .child(action_button(
                    "preview-redo",
                    IconName::Redo2,
                    "重做 (Ctrl+Y)",
                    Redo,
                ))
                .child(toolbar_separator(cx))
                .child(action_button(
                    "preview-cut",
                    IconName::Scissors,
                    "剪切 (Ctrl+X)",
                    Cut,
                ))
                .child(action_button(
                    "preview-copy",
                    IconName::Copy,
                    "复制 (Ctrl+C)",
                    Copy,
                ))
                .child(action_button(
                    "preview-paste",
                    IconName::ClipboardPaste,
                    "粘贴 (Ctrl+V)",
                    Paste,
                ))
                .child(toolbar_separator(cx))
                .child(action_button(
                    "preview-find",
                    IconName::Search,
                    "查找 (Ctrl+F)",
                    Search,
                ))
                .child(action_button(
                    "preview-replace",
                    IconName::Replace,
                    "替换 (Ctrl+H)",
                    Replace,
                ))
                .child(
                    Button::new("preview-goto-line")
                        .ghost()
                        .xsmall()
                        .icon(crate::assets::ui_icon(IconName::Hash))
                        .tooltip("跳转到行")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.open_goto_line(window, cx)),
                        ),
                )
                .child(toolbar_separator(cx))
                .child(
                    Button::new("preview-soft-wrap")
                        .ghost()
                        .xsmall()
                        .icon(crate::assets::ui_icon(IconName::TextWrap))
                        .tooltip("自动换行")
                        .selected(text.soft_wrap)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.toggle_soft_wrap(window, cx)),
                        ),
                );
        }

        bar
    }

    /// 标签条:**一个文件一枚标签**(见模块文档),标签上是文件名 + 关闭按钮。
    ///
    /// 用 gpui-kit 的 [`TabBar`] / [`Tab`]:`TabBar::max_width` 会给标签宽度封顶、长名字省略
    /// 中段而**保住后面的关闭按钮**(那两条都有单测盯着),自己搭一套很难不把按钮挤没。
    /// 点某一枚标签 = 切到那个标签(`TabBar::on_click` 给下标);标签上的 × 只关**它自己**
    /// (⚠️ `TabBar::on_click` 会盖掉 `Tab` 自己的 `on_click`,所以 × 的关闭手势由它兜住)。
    pub(super) fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut tabs = Vec::with_capacity(self.tabs.len());
        for (ix, tab) in self.tabs.iter().enumerate() {
            let dirty = matches!(&tab.body, Body::Text(text) if text.is_dirty());
            let description = format!("{}:{}", tab.request.host, tab.request.remote_path);
            let mut built =
                Tab::new()
                    .label(tab.request.name.clone())
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(description.clone())
                            .build(window, cx)
                    });
            if dirty {
                // 有没写回的改动:名字前面点一颗(状态栏那行小字写的是「写回成功 / 失败」,这里写的是
                // 「还有东西没写回」)。
                built = built.prefix(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(cx.theme().primary)
                        .child("●"),
                );
            }
            // 每一枚 × 只关**它自己**(最后一个也会走 `close_tab`,那里再决定要不要连窗口一起收)。
            // ⚠️ 必须 `stop_propagation`:× 是标签的子节点,点它在冒泡阶段还会撞上标签自己的
            // `on_click`(`TabBar::on_click` 那条「切标签」),不拦就会「先关掉这个标签、又被当成
            // 点了它去切标签」(那时下标已经错位)。
            let close = cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.request_close_tab(ix, window, cx)
            });
            tabs.push(
                built.suffix(
                    Button::new(("preview-tab-close", ix))
                        .ghost()
                        .xsmall()
                        .icon(crate::assets::ui_icon(IconName::WindowClose))
                        .tooltip("关闭标签 (Ctrl+W)")
                        .occlude()
                        .on_click(close),
                ),
            );
        }

        h_flex()
            .w_full()
            .flex_none()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                TabBar::new("preview-tabs")
                    .small()
                    .max_width(px(260.))
                    .selected_index(self.active)
                    .on_click(cx.listener(|this, ix, window, cx| this.select_tab(*ix, window, cx)))
                    .children(tabs),
            )
    }

    /// 正文:当前标签的正文。
    pub(super) fn render_body(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(tab) = self.active() else {
            return div().size_full().into_any_element();
        };
        match &tab.body {
            Body::Loading => centered(
                crate::assets::ui_icon(IconName::FileText),
                "正在读取…",
                None,
                cx,
            )
            .into_any_element(),
            // ⚠️ `size_full()` 不能省。多行编辑器没给高度时按**内容行数**撑高,而
            // `LayoutMode::CodeEditor` 的默认 `rows` 是 **2**(见 gpui-kit 的 `mode.rs` 与
            // `input.rs` 里那个 `h_auto()` 分支)⇒ 编辑器会缩成两行、后面的内容全被裁掉,
            // 滚动条还会压在第二行上。官方 story 也是显式 `.size_full()` 才铺满的。
            // 这里**不给** `.readonly(..)`:文本是能改的,改完 Ctrl+S 写回远端(见 `Self::save`)。
            Body::Text(text) => Editor::new(&text.editor)
                .appearance(false)
                .bordered(false)
                .rounded_none()
                .p_0()
                .size_full()
                .into_any_element(),
            Body::Failed(message) => centered(
                crate::assets::ui_icon(IconName::TriangleAlert),
                "打不开这个文件",
                Some(message.as_str()),
                cx,
            )
            .child({
                let id = tab.id;
                Button::new("editor-retry-load").label("重试").on_click(
                    cx.listener(move |this, _, window, cx| this.reload_tab(id, window, cx)),
                )
            })
            .child({
                let request = tab.request.clone();
                Button::new("preview-open-externally")
                    .label("使用系统程序打开")
                    .on_click(move |_, _, cx| crate::file_open::open_in_system(request.clone(), cx))
            })
            .into_any_element(),
        }
    }

    /// 底部状态栏:左边是「主机:远端路径」(+ 大小 + 写回状态),右边是语言 / 编码 /
    /// 光标位置 / 行数字符数(照普通文本编辑器的摆法)。都是**当前标签**的。
    ///
    /// 与主窗口共用状态栏布局和组件样式。右侧信息每帧从编辑器读取，光标移动时编辑器会
    /// `cx.notify()`(订阅见 [`EditorWindow::show_text`]),所以它们跟着光标走。
    pub(super) fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(tab) = self.active() else {
            return new_status_bar()
                .child(div().h(STATUS_BAR_ITEM_HEIGHT))
                .into_any_element();
        };
        // 主机名来自建会话时的参数(见 [`FileOpenRequest::host`])——远端那份到底是哪台机器上的,
        // 只有这一处说。大小与写回状态都挨着它摆(⚠️ `StatusBar` 的 `left` / `right` 是两端
        // 各自一条,`child` 才是中间那段;放在中间那段的元素才有地方省略,所以整簇走 `child`)。
        let mut left = h_flex()
            .min_h(STATUS_BAR_ITEM_HEIGHT)
            .flex_1()
            .min_w_0()
            .gap_2()
            .items_center()
            .child(
                crate::assets::ui_icon(IconName::Globe)
                    .flex_none()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(format!("{}:{}", tab.request.host, tab.request.remote_path)),
            );
        if tab.request.size > 0 {
            left = left.child(div().flex_none().child(format::size(tab.request.size)));
        }
        if let Body::Text(text) = &tab.body {
            // 写回状态:「写回中 42%」/「已写回远端」/「写回失败…」。
            if let Some((label, failed)) = save_label(text) {
                left = left.child(
                    div()
                        .flex_none()
                        .text_color(if failed {
                            cx.theme().danger
                        } else {
                            cx.theme().muted_foreground
                        })
                        .child(label),
                );
            }
        }

        let mut bar = new_status_bar().child(left);

        match &tab.body {
            Body::Text(text) => {
                let state = text.editor.read(cx);
                let (row, col, pos) = caret(state.text(), state.cursor());
                let (lines, chars) = counts(state.text());
                bar = bar
                    .right(text.language.label())
                    .right(text.document.format.encoding())
                    .right(text.document.format.line_endings())
                    .right(format!("Row: {row} | Col: {col} | Pos: {pos}"))
                    .right(format!("{lines} lines | {chars} chars"));
            }
            // 没有编辑器的:至少把类型标出来(大小在左边那簇里)。
            _ => {
                bar = bar.right("文本");
            }
        }
        bar.into_any_element()
    }
}

impl Focusable for EditorWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.root_focus.clone()
    }
}

impl Render for EditorWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            // 窗口级动作全绑在 keymap 的 `"Preview"` context 上(见 `crate::keymap` 与
            // `assets/keymaps/default-*.json`)。⚠️ 这些动作**只由本窗口自己接**:它们是窗口级的
            // (预览窗口只有一个,里面有多少标签由本窗口自己管),不能像别的动作那样丢给
            // `AppRoot` 的全局监听器 —— 全局那侧拿不到「是哪个预览窗口」。
            .key_context(PREVIEW_CONTEXT)
            .track_focus(&self.root_focus)
            .on_action(cx.listener(|this, _: &SavePreviewFile, window, cx| this.save(window, cx)))
            .on_action(cx.listener(|this, _: &ClosePreviewWindow, window, cx| {
                this.request_close_self(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &GotoPreviewLine, window, cx| {
                    this.open_goto_line(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &TogglePreviewWordWrap, window, cx| {
                this.toggle_soft_wrap(window, cx)
            }))
            // 顶部:自绘标题栏 —— **只有窗口控制按钮**(`TitleBar` 自己给);不放文件名,也不再
            // 嵌菜单栏(见模块文档):当前文件叫什么在标签条与状态栏上,窗口标题由
            // `set_window_title` 另设。
            .child(TitleBar::new())
            .child(self.render_toolbar(cx))
            .child(self.render_tab_bar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(self.render_body(cx)),
            )
            .child(self.render_status_bar(cx))
        // 覆盖层由 gpui-kit `Root` 渲染。
    }
}
