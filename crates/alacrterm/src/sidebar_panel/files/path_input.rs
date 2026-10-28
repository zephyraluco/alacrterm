//! 路径编辑、远端目录候选及确认导航。候选查询不修改文件树。

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use gpui::{
    AnyElement, App, Context, Entity, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, ScrollHandle, StatefulInteractiveElement as _, Styled as _, Window, div,
    px,
};
use gpui_kit::component::{
    ActiveTheme as _, WindowExt as _,
    button::Button,
    dialog::{DialogAction, DialogFooter},
    input::{Escape, IndentInline, Input, InputEvent, MoveDown, MoveUp},
    list::ListItem,
    v_flex,
};
use terminal::RemoteEntry;

use super::{FilesState, PATH_ROW_HEIGHT, join_dir};

const QUERY_DELAY: Duration = Duration::from_millis(200);
const CACHE_TTL: Duration = Duration::from_secs(10);
const CACHE_LIMIT: usize = 16;
const ROW_HEIGHT: f32 = 28.;

type CachedDirectory = (String, Instant, Vec<RemoteEntry>);

#[derive(Default)]
pub(super) struct PathCompletion {
    pub(super) editing: bool,
    pub(super) value: String,
    pub(super) base: Option<String>,
    seq: u64,
    open: bool,
    loading: bool,
    message: Option<String>,
    candidates: Vec<String>,
    selected: Option<usize>,
    scroll: ScrollHandle,
    pub(super) cache: VecDeque<CachedDirectory>,
}

impl PathCompletion {
    fn set_candidates(&mut self, candidates: Vec<String>) {
        self.selected = (!candidates.is_empty()).then_some(0);
        self.candidates = candidates;
    }

    fn selected_path(&self) -> Option<&str> {
        if !self.open || self.loading {
            return None;
        }
        self.candidates.get(self.selected?).map(String::as_str)
    }

    pub(super) fn dismiss(&mut self) {
        self.seq += 1;
        self.open = false;
        self.candidates.clear();
        self.selected = None;
        self.loading = false;
    }
}

/// 只展开路径前缀；`.` / `..` 必须交给远端解析，避免改变符号链接后的路径语义。
pub(super) fn resolve_path(text: &str, root: Option<&str>, home: Option<&str>) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let absolute = if text == "~" {
        home?.to_string()
    } else if let Some(rest) = text.strip_prefix("~/") {
        join_dir(home?, rest)
    } else if text.starts_with('/') {
        text.to_string()
    } else {
        join_dir(root.or(home)?, text)
    };
    Some(absolute)
}

fn query_parts(text: &str, root: Option<&str>, home: Option<&str>) -> Option<(String, String)> {
    let path = resolve_path(text, root, home)?;
    if text.trim().ends_with('/') || text.trim() == "~" || path == "/" {
        return Some((path, String::new()));
    }
    let (parent, prefix) = path.rsplit_once('/')?;
    Some((
        if parent.is_empty() {
            "/".into()
        } else {
            parent.into()
        },
        prefix.into(),
    ))
}

fn directory_candidates(dir: &str, prefix: &str, entries: &[RemoteEntry]) -> Vec<String> {
    let mut paths: Vec<_> = entries
        .iter()
        .filter(|entry| {
            entry.is_dir
                && entry.name != "."
                && entry.name != ".."
                && entry.name.starts_with(prefix)
        })
        .map(|entry| format!("{}/", join_dir(dir, &entry.name)))
        .collect();
    paths.sort();
    paths
}

impl FilesState {
    pub(super) fn on_path_input_event(
        &mut self,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                let text = self.path_input.read(cx).value().to_string();
                // set_value 同样发出 Change；同步目录显示不应重新打开候选浮层。
                if self.completion.value == text {
                    return;
                }
                self.completion.value = text;
                self.completion.editing = true;
                self.path_input_pending = false;
                self.nav_seq += 1;
                self.query_candidates(cx);
            }
            InputEvent::PressEnter { secondary, shift } => {
                if !secondary && !shift && self.path_command("enter", window, cx) {
                    return;
                }
                let text = self.path_input.read(cx).value().to_string();
                self.navigate_to(&text, cx);
            }
            InputEvent::Focus => self.completion.editing = true,
            InputEvent::Blur => {
                self.completion.editing = false;
                self.completion.dismiss();
                cx.notify();
            }
        }
    }

    fn set_path_text(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.completion.value = text.clone();
        self.path_input
            .update(cx, |input, cx| input.set_value(text, window, cx));
    }

    fn complete_path(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.completion.editing = true;
        self.path_input_pending = false;
        self.nav_seq += 1;
        self.set_path_text(path, window, cx);
        self.query_candidates(cx);
    }

    fn query_candidates(&mut self, cx: &mut Context<Self>) {
        self.completion.dismiss();
        let Some(fs) = self.fs.clone() else {
            return;
        };
        let Some((dir, prefix)) = query_parts(
            &self.completion.value,
            self.root.as_deref().or(self.completion.base.as_deref()),
            self.home.as_deref(),
        ) else {
            cx.notify();
            return;
        };
        self.completion.open = true;
        self.completion.message = None;
        self.completion
            .scroll
            .set_offset(gpui::point(px(0.), px(0.)));
        if let Some((_, _, entries)) = self
            .completion
            .cache
            .iter()
            .find(|(path, time, _)| *path == dir && time.elapsed() < CACHE_TTL)
        {
            let candidates = directory_candidates(&dir, &prefix, entries);
            self.completion.set_candidates(candidates);
            cx.notify();
            return;
        }
        self.completion.loading = true;
        let seq = self.completion.seq;
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(QUERY_DELAY).await;
            let current = this
                .read_with(cx, |this, _| {
                    this.completion.seq == seq && this.generation == generation
                })
                .unwrap_or(false);
            if !current {
                return;
            }
            let result = fs.list_completion_dir(dir.clone()).await;
            let _ = this.update(cx, |this, cx| {
                if this.completion.seq != seq || this.generation != generation {
                    return;
                }
                this.completion.loading = false;
                match result {
                    Ok(entries) => {
                        this.completion
                            .set_candidates(directory_candidates(&dir, &prefix, &entries));
                        this.completion.cache.retain(|(path, _, _)| path != &dir);
                        this.completion
                            .cache
                            .push_back((dir, Instant::now(), entries));
                        while this.completion.cache.len() > CACHE_LIMIT {
                            this.completion.cache.pop_front();
                        }
                    }
                    Err(error) => {
                        this.completion.message = Some(format!("无法读取候选目录：{error}"))
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn path_command(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.completion.open {
            return false;
        }
        match key {
            "escape" => self.completion.dismiss(),
            "up" | "down" if self.completion.selected_path().is_some() => {
                let count = self.completion.candidates.len();
                let ix = match (self.completion.selected, key) {
                    (Some(ix), "up") => (ix + count - 1) % count,
                    (Some(ix), _) => (ix + 1) % count,
                    (None, "up") => count - 1,
                    (None, _) => 0,
                };
                self.completion.selected = Some(ix);
                self.completion.scroll.scroll_to_item(ix);
            }
            "tab" | "enter" => {
                let Some(path) = self.completion.selected_path().map(str::to_owned) else {
                    return false;
                };
                self.complete_path(path, window, cx);
            }
            _ => return false,
        }
        window.prevent_default();
        cx.stop_propagation();
        cx.notify();
        true
    }

    pub(in crate::sidebar_panel) fn render_path_input(
        &self,
        state: Entity<Self>,
        cx: &App,
    ) -> Option<AnyElement> {
        self.fs.as_ref()?;
        let popup = (self.completion.open && !self.completion.loading).then(|| {
            let mut rows = Vec::new();
            for (ix, path) in self.completion.candidates.iter().enumerate() {
                let selected = self.completion.selected == Some(ix);
                let state = state.clone();
                let path = path.clone();
                rows.push(
                    ListItem::new(("path-candidate", ix))
                        .h(px(ROW_HEIGHT))
                        .flex_none()
                        .py_0()
                        .px_2()
                        .w_full()
                        .selected(selected)
                        .child(format!(
                            "{}/",
                            path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
                        ))
                        .on_mouse_down(MouseButton::Left, |_, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                        })
                        .on_click(move |_, window, cx| {
                            state.update(cx, |this, cx| {
                                if this.completion.open
                                    && !this.completion.loading
                                    && this.completion.candidates.contains(&path)
                                {
                                    this.complete_path(path.clone(), window, cx);
                                }
                            })
                        })
                        .into_any_element(),
                );
            }
            let message = if self.completion.message.is_some() {
                self.completion.message.clone()
            } else if rows.is_empty() {
                Some("没有匹配的目录".to_string())
            } else {
                None
            };
            if let Some(message) = message {
                rows.push(div().px_2().py_1().child(message).into_any_element());
            }
            gpui::deferred(
                div()
                    .absolute()
                    .top_full()
                    .left_0()
                    .w_full()
                    .occlude()
                    .bg(cx.theme().popover)
                    .text_color(cx.theme().popover_foreground)
                    .text_xs()
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(px(4.))
                    .shadow_md()
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                    })
                    .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                    .child(
                        v_flex()
                            .id("path-candidates")
                            .max_h(px(ROW_HEIGHT * 8.))
                            .overflow_y_scroll()
                            .track_scroll(&self.completion.scroll)
                            .children(rows),
                    ),
            )
            .into_any_element()
        });
        Some(
            div()
                .relative()
                .w_full()
                .h(PATH_ROW_HEIGHT)
                .flex_none()
                .capture_action(completion_action::<MoveUp>(state.clone(), "up"))
                .capture_action(completion_action::<MoveDown>(state.clone(), "down"))
                .capture_action(completion_action::<Escape>(state.clone(), "escape"))
                .capture_action(completion_action::<IndentInline>(state.clone(), "tab"))
                .child(
                    Input::new(&self.path_input)
                        .cleanable(false)
                        .aria_label("远端目录路径"),
                )
                .children(popup)
                .into_any_element(),
        )
    }

    pub(super) fn show_navigation_error(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(message) = self.navigation_error.take() else {
            return;
        };
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("路径跳转失败")
                .w(px(420.))
                .child(div().text_sm().child(message.clone()))
                .footer(
                    DialogFooter::new().child(
                        DialogAction::new().child(Button::new("path-error-ok").label("确定")),
                    ),
                )
        });
    }
}

// 输入组件先分派动作，再分派原始按键；在捕获阶段拦截才能保留输入焦点。
fn completion_action<A: gpui::Action>(
    state: Entity<FilesState>,
    key: &'static str,
) -> impl Fn(&A, &mut Window, &mut App) {
    move |_, window, cx| {
        state.update(cx, |this, cx| {
            if !this.path_command(key, window, cx) {
                cx.propagate();
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_results_select_first_directory_and_reset_old_selection() {
        let mut completion = PathCompletion::default();
        completion.open = true;
        completion.set_candidates(vec!["/alpha/".into(), "/beta/".into()]);
        assert_eq!(completion.selected_path(), Some("/alpha/"));
        completion.selected = Some(1);
        completion.set_candidates(vec!["/other/".into()]);
        assert_eq!(completion.selected_path(), Some("/other/"));
        completion.set_candidates(Vec::new());
        assert_eq!(completion.selected_path(), None);
    }

    #[test]
    fn pending_or_dismissed_candidates_cannot_be_accepted() {
        let mut completion = PathCompletion::default();
        completion.open = true;
        completion.set_candidates(vec!["/old/".into()]);
        completion.loading = true;
        assert_eq!(completion.selected_path(), None);
        completion.loading = false;
        assert_eq!(completion.selected_path(), Some("/old/"));
        completion.dismiss();
        assert_eq!(completion.selected_path(), None);
    }

    #[test]
    fn resolves_absolute_home_and_relative_posix_paths() {
        assert_eq!(
            resolve_path("/usr/../var/", None, None).as_deref(),
            Some("/usr/../var/")
        );
        assert_eq!(
            resolve_path("~/code", None, Some("/home/me")).as_deref(),
            Some("/home/me/code")
        );
        assert_eq!(
            resolve_path("../logs", Some("/srv/app"), None).as_deref(),
            Some("/srv/app/../logs")
        );
        assert_eq!(resolve_path("~", None, None), None);
        assert_eq!(resolve_path("", Some("/tmp"), None), None);
    }

    #[test]
    fn preserves_symlink_parent_components_for_navigation_and_completion() {
        assert_eq!(
            resolve_path("/home/me/link/..", None, None).as_deref(),
            Some("/home/me/link/..")
        );
        assert_eq!(
            resolve_path("~/link/..", None, Some("/home/me")).as_deref(),
            Some("/home/me/link/..")
        );
        assert_eq!(
            resolve_path("link/../logs", Some("/home/me"), None).as_deref(),
            Some("/home/me/link/../logs")
        );
        assert_eq!(
            query_parts("/home/me/link/../proj", None, None),
            Some(("/home/me/link/..".into(), "proj".into()))
        );
    }

    #[cfg(unix)]
    #[test]
    fn parent_after_symlink_resolves_to_the_target_parent() {
        let dir = tempfile::tempdir().expect("temporary filesystem");
        let home = dir.path().join("home/me");
        let server = dir.path().join("srv");
        let project = server.join("project");
        std::fs::create_dir_all(&home).expect("home directory");
        std::fs::create_dir_all(&project).expect("target directory");
        let expected = std::fs::canonicalize(&server).expect("canonical target parent");
        std::os::unix::fs::symlink(&project, home.join("link")).expect("remote symlink fixture");
        let input = format!("{}/link/..", home.display());
        let path = resolve_path(&input, None, None).expect("absolute path");
        assert_eq!(
            std::fs::canonicalize(path).expect("server resolves path"),
            expected
        );
        let (parent, prefix) =
            query_parts(&format!("{input}/proj"), None, None).expect("completion path");
        assert_eq!(
            std::fs::canonicalize(parent).expect("server resolves query"),
            expected
        );
        assert_eq!(prefix, "proj");
    }

    #[test]
    fn queries_parent_prefix_or_children_after_slash() {
        assert_eq!(
            query_parts("/usr/lo", None, None),
            Some(("/usr".into(), "lo".into()))
        );
        assert_eq!(
            query_parts("/usr/", None, None),
            Some(("/usr/".into(), "".into()))
        );
        assert_eq!(
            query_parts("./", Some("/srv"), None),
            Some(("/srv/./".into(), "".into()))
        );
        assert_eq!(
            query_parts("~/", None, Some("/home/me")),
            Some(("/home/me/".into(), "".into()))
        );
    }

    #[test]
    fn filters_files_dot_entries_and_sorts_matching_directories() {
        let entries = [
            ("local", true),
            ("lost", true),
            ("lo.txt", false),
            ("bin", true),
            ("..", true),
        ]
        .map(|(name, is_dir)| RemoteEntry {
            name: name.into(),
            is_dir,
            size: 0,
        });
        assert_eq!(
            directory_candidates("/usr", "lo", &entries),
            ["/usr/local/", "/usr/lost/"]
        );
        assert!(!directory_candidates("/", "", &entries).contains(&"/../".into()));
    }
}
