//! 应用自定义 Action：供右键菜单等「官方 action 风格」的菜单项使用。
//!
//! 按 gpui-kit 官方示例，菜单项写作
//! `menu.menu("标签", Box::new(SomeAction))`，点击后由菜单内部
//! `window.dispatch_action(...)` 派发。接收方有两种写法：
//!
//! - **元素级** `element.on_action(cx.listener(|this, action: &A, window, cx| ...))`：
//!   沿「焦点路径」派发，依赖当前焦点位置；
//! - **全局级** [`App::on_action`]（本模块采用）：在 action 冒泡阶段**一定**会被调用，
//!   与焦点无关。
//!
//! 菜单是**同一窗口内的浮层**（`ContextMenuExt` 用 `deferred` 渲染），但它派发 action 时
//! 若没有设置 `action_context`，焦点可能仍在终端上；用全局监听器可避免这层不确定性。
//! 代价是全局监听器只有 `&mut App`（没有窗口、也没有根视图），因此回调里要配合
//! [`AppRoot::defer_after_update`] 才能做「需要窗口」的操作。

use gpui::{Action, App, WeakEntity, actions};
use serde::Deserialize;

use crate::AppRoot;

actions!(alacrterm, [NewTerminal, OpenSettings]);

/// 关闭指定下标的终端会话。
///
/// 带数据的 action 需要 `Deserialize`（`Action` trait 的约束），
/// `no_json` 则省掉 `JsonSchema` 依赖。
#[derive(Clone, Action, PartialEq, Eq, Deserialize)]
#[action(namespace = alacrterm, no_json)]
pub(crate) struct CloseSession {
    pub(crate) index: usize,
}

impl AppRoot {
    /// 注册全局 action 监听器（应在窗口创建后调用一次）。
    ///
    /// 放在这里而不是 `AppRoot::new` 里：那里拿不到已构造好的实体句柄，
    /// 而 `main` 的建窗闭包里正好有 `Entity<AppRoot>`。
    pub(crate) fn register_actions(root: WeakEntity<Self>, cx: &mut App) {
        // —— 关闭会话：不需要窗口，直接 update 即可 ——
        let close_root = root.clone();
        cx.on_action(move |action: &CloseSession, cx: &mut App| {
            let index = action.index;
            let _ = close_root.update(cx, |this, cx| this.close_terminal(index, cx));
        });

        // —— 新建终端：要开对话框（需要窗口），让出一拍再执行 ——
        let open_root = root.clone();
        cx.on_action(move |_: &NewTerminal, cx: &mut App| {
            Self::defer_after_update(root.clone(), cx, |this, window, cx| {
                this.open_new_terminal_dialog(window, cx)
            });
        });

        // —— 打开设置：同样需要窗口（开新窗口），也走 defer ——
        cx.on_action(move |_: &OpenSettings, cx: &mut App| {
            Self::defer_after_update(open_root.clone(), cx, |this, window, cx| {
                this.open_settings_window(window, cx)
            });
        });
    }
}
