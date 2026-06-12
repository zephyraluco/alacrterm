use super::{HoverTarget, HoveredWord, TerminalView};
use gpui::{Context, Task, Window};
use terminal::PathLikeTarget;

pub(super) fn hover_path_like_target(
    hovered_word: HoveredWord,
    path_like_target: &PathLikeTarget,
    cx: &mut Context<TerminalView>,
) -> Task<()> {
    let tooltip = path_like_target
        .terminal_dir
        .as_ref()
        .map(|terminal_dir| terminal_dir.join(&path_like_target.maybe_path))
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| path_like_target.maybe_path.clone());

    cx.spawn(async move |terminal_view, cx| {
        terminal_view
            .update(cx, |terminal_view, cx| {
                terminal_view.hover = Some(HoverTarget {
                    tooltip,
                    hovered_word,
                });
                cx.notify();
            })
            .ok();
    })
}

pub(super) fn open_path_like_target(
    path_like_target: &PathLikeTarget,
    window: &mut Window,
    cx: &mut Context<TerminalView>,
) {
    let _ = path_like_target;
    let _ = window;
    cx.notify();
}
