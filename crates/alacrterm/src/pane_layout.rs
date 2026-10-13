//! 分屏布局：一个标签页内部的**格子树**（kitty / ghostty 风格）。
//!
//! 每次分屏把一片叶子换成 `Split`（二叉），关格时摘掉叶子并把「只剩一个孩子」的
//! 分叉折叠掉。全局固定 50/50，不做拖拽调尺寸，所以树里不存比例 —— 两侧都是
//! `flex_1`，中间的 1px 分隔线只负责让人看出分了屏。
//!
//! 叶子存的是**下标**（进 [`crate::terminal_panel::TabPane::panes`]），所以摘掉一片后
//! 要把比它大的下标全部前移一位。

use gpui::{
    AnyElement, Axis, Entity, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    Styled as _, div, px,
};
use gpui_kit::base::{h_flex, v_flex};
use terminal_view::TerminalView;

/// 分隔线粗细。
const DIVIDER: gpui::Pixels = px(1.);

/// 焦点移动方向（`Ctrl+Shift+方向键`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// 分屏树。
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Layout {
    /// 一块格子（下标进标签页的 `panes`）。
    Leaf(usize),
    /// 一次二分割：`first` 在左 / 上，`second` 在右 / 下。
    Split {
        axis: Axis,
        first: Box<Layout>,
        second: Box<Layout>,
    },
}

/// 单位坐标（`0..1`，相对于整个标签页的内容区），只用来看相对方位。
#[derive(Clone, Copy, Debug)]
struct UnitRect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl UnitRect {
    fn root() -> Self {
        Self {
            x: 0.,
            y: 0.,
            w: 1.,
            h: 1.,
        }
    }

    fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2., self.y + self.h / 2.)
    }
}

impl Layout {
    /// 单个格子的树。
    pub(crate) fn single() -> Self {
        Layout::Leaf(0)
    }

    /// 把 [`Layout::Leaf`]`(target)` 换成 `Split { Leaf(target), Leaf(new_ix) }`
    /// ——新格子落在目标格子的**右 / 下**。
    ///
    /// 返回 `false` 表示树里没有 `target`（调用方不应该把新格子登记进面板表）。
    pub(crate) fn split(&mut self, target: usize, axis: Axis, new_ix: usize) -> bool {
        let Some(node) = leaf_mut(self, target) else {
            return false;
        };
        let old = std::mem::replace(node, Layout::Leaf(usize::MAX));
        *node = Layout::Split {
            axis,
            first: Box::new(old),
            second: Box::new(Layout::Leaf(new_ix)),
        };
        true
    }

    /// 摘掉下标为 `ix` 的叶子，并把剩下的下标前移一位。
    ///
    /// 调用方保证树里还剩至少一片叶子（关最后一格是关整个标签页，不走这里）。
    pub(crate) fn remove(&mut self, ix: usize) {
        let old = std::mem::replace(self, Layout::Leaf(usize::MAX));
        if let Some(pruned) = pruned(old, ix) {
            *self = pruned;
            shift_after_removal(self, ix);
        }
    }

    /// 方向导航：`from` 那个格子往 `dir` 去的邻居。
    ///
    /// 只认「沿该轴在前 + 另一轴真的重叠」的格子，取轴向最近的那个
    /// （轴向相同再比另一轴的距离）。没有就返回 `None`（不折返）。
    pub(crate) fn neighbor(&self, from: usize, dir: Dir) -> Option<usize> {
        let rects = self.rects();
        let current = rects.iter().find(|(ix, _)| *ix == from)?.1;
        let (cx, cy) = current.center();

        let mut best: Option<(f32, f32, usize)> = None;
        for (ix, rect) in &rects {
            if *ix == from {
                continue;
            }
            let (px, py) = rect.center();
            let along = match dir {
                Dir::Left if px < cx => cx - px,
                Dir::Right if px > cx => px - cx,
                Dir::Up if py < cy => cy - py,
                Dir::Down if py > cy => py - cy,
                _ => continue,
            };
            // 「斜对角」不算邻居：另一轴得真的和当前格子有交集。
            let overlaps = match dir {
                Dir::Left | Dir::Right => {
                    rect.y < current.y + current.h && current.y < rect.y + rect.h
                }
                Dir::Up | Dir::Down => {
                    rect.x < current.x + current.w && current.x < rect.x + rect.w
                }
            };
            if !overlaps {
                continue;
            }
            let cross = match dir {
                Dir::Left | Dir::Right => (cy - py).abs(),
                Dir::Up | Dir::Down => (cx - px).abs(),
            };
            let better = best.is_none_or(|(best_along, best_cross, _)| {
                (along, cross) < (best_along, best_cross)
            });
            if better {
                best = Some((along, cross, *ix));
            }
        }
        best.map(|(_, _, ix)| ix)
    }

    /// 每片叶子的单位坐标。
    fn rects(&self) -> Vec<(usize, UnitRect)> {
        let mut out = Vec::new();
        collect_rects(self, UnitRect::root(), &mut out);
        out
    }

    /// 渲染整棵树；叶子渲染成 [`TerminalView`]，分叉渲染成两侧 `flex_1` + 中间分隔线。
    pub(crate) fn render(&self, panes: &[Entity<TerminalView>], divider: Hsla) -> AnyElement {
        match self {
            Layout::Leaf(ix) => pane_element(panes, *ix),
            Layout::Split { axis, first, second } => {
                let first = first.render(panes, divider);
                let second = second.render(panes, divider);
                match axis {
                    Axis::Horizontal => h_flex()
                        .size_full()
                        .min_w_0()
                        .child(div().flex_1().min_w_0().h_full().child(first))
                        .child(div().flex_none().w(DIVIDER).h_full().bg(divider))
                        .child(div().flex_1().min_w_0().h_full().child(second))
                        .into_any_element(),
                    Axis::Vertical => v_flex()
                        .size_full()
                        .min_h_0()
                        .child(div().flex_1().min_h_0().w_full().child(first))
                        .child(div().flex_none().h(DIVIDER).w_full().bg(divider))
                        .child(div().flex_1().min_h_0().w_full().child(second))
                        .into_any_element(),
                }
            }
        }
    }
}

/// 一块格子。
///
/// ⚠️ 外层 div 必须带 **每个终端唯一** 的 id：`TerminalElement` 自己不贡献 element id
/// （`Interactivity` 是 `default()`），同一页里两块终端会算出同一条 global id 路径。
fn pane_element(panes: &[Entity<TerminalView>], ix: usize) -> AnyElement {
    let Some(view) = panes.get(ix) else {
        return div().size_full().into_any_element();
    };
    div()
        .id(("terminal-pane", view.entity_id()))
        .size_full()
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
        .child(view.clone())
        .into_any_element()
}

/// 找到并借出「就是 `Leaf(ix)`」的那个节点。
fn leaf_mut(layout: &mut Layout, ix: usize) -> Option<&mut Layout> {
    if matches!(layout, Layout::Leaf(i) if *i == ix) {
        return Some(layout);
    }
    match layout {
        Layout::Leaf(_) => None,
        Layout::Split { first, second, .. } => {
            leaf_mut(first, ix).or_else(|| leaf_mut(second, ix))
        }
    }
}

/// 摘掉叶子 `ix`；`None` = 这棵子树整个没了，`Some` = 剩下的部分（只剩一边时直接顶上来）。
fn pruned(layout: Layout, ix: usize) -> Option<Layout> {
    match layout {
        Layout::Leaf(i) if i == ix => None,
        Layout::Leaf(i) => Some(Layout::Leaf(i)),
        Layout::Split { axis, first, second } => {
            match (pruned(*first, ix), pruned(*second, ix)) {
                (Some(first), Some(second)) => Some(Layout::Split {
                    axis,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(only), None) | (None, Some(only)) => Some(only),
                (None, None) => None,
            }
        }
    }
}

/// 摘掉 `removed` 之后，比它大的下标全部前移一位。
fn shift_after_removal(layout: &mut Layout, removed: usize) {
    match layout {
        Layout::Leaf(i) => {
            if *i > removed {
                *i -= 1;
            }
        }
        Layout::Split { first, second, .. } => {
            shift_after_removal(first, removed);
            shift_after_removal(second, removed);
        }
    }
}

fn collect_rects(layout: &Layout, rect: UnitRect, out: &mut Vec<(usize, UnitRect)>) {
    match layout {
        Layout::Leaf(i) => out.push((*i, rect)),
        Layout::Split { axis, first, second } => match axis {
            Axis::Horizontal => {
                let w = rect.w / 2.;
                collect_rects(first, UnitRect { w, ..rect }, out);
                collect_rects(
                    second,
                    UnitRect {
                        x: rect.x + w,
                        w,
                        ..rect
                    },
                    out,
                );
            }
            Axis::Vertical => {
                let h = rect.h / 2.;
                collect_rects(first, UnitRect { h, ..rect }, out);
                collect_rects(
                    second,
                    UnitRect {
                        y: rect.y + h,
                        h,
                        ..rect
                    },
                    out,
                );
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(ix: usize) -> Layout {
        Layout::Leaf(ix)
    }

    fn split(axis: Axis, first: Layout, second: Layout) -> Layout {
        Layout::Split {
            axis,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    #[test]
    fn split_puts_the_new_pane_after_the_target() {
        let mut layout = Layout::single();
        layout.split(0, Axis::Horizontal, 1);
        assert_eq!(layout, split(Axis::Horizontal, leaf(0), leaf(1)));
    }

    #[test]
    fn split_targets_a_nested_leaf() {
        let mut layout = split(Axis::Horizontal, leaf(0), leaf(1));
        layout.split(1, Axis::Vertical, 2);
        assert_eq!(
            layout,
            split(Axis::Horizontal, leaf(0), split(Axis::Vertical, leaf(1), leaf(2)))
        );
    }

    #[test]
    fn removing_a_leaf_collapses_its_parent() {
        let mut layout = split(Axis::Horizontal, leaf(0), leaf(1));
        layout.remove(0);
        assert_eq!(layout, leaf(0));
    }

    #[test]
    fn removing_renumbers_the_leaves_after_it() {
        let mut layout = split(Axis::Horizontal, leaf(0), split(Axis::Vertical, leaf(1), leaf(2)));
        layout.remove(1);
        // 原来的 2 号前移成 1 号，左边的 0 号不动。
        assert_eq!(layout, split(Axis::Horizontal, leaf(0), leaf(1)));
    }

    #[test]
    fn removing_the_last_leaf_keeps_the_tree_prefix() {
        let mut layout = split(Axis::Horizontal, leaf(0), split(Axis::Vertical, leaf(1), leaf(2)));
        layout.remove(2);
        assert_eq!(layout, split(Axis::Horizontal, leaf(0), leaf(1)));
    }

    #[test]
    fn neighbor_walks_the_axes() {
        //  ┌───────┬───────┐
        //  │   0   │   1   │
        //  │       ├───────┤
        //  │       │   2   │
        //  └───────┴───────┘
        let layout = split(Axis::Horizontal, leaf(0), split(Axis::Vertical, leaf(1), leaf(2)));
        assert_eq!(layout.neighbor(0, Dir::Right), Some(1));
        assert_eq!(layout.neighbor(1, Dir::Left), Some(0));
        assert_eq!(layout.neighbor(1, Dir::Down), Some(2));
        assert_eq!(layout.neighbor(2, Dir::Up), Some(1));
        assert_eq!(layout.neighbor(0, Dir::Left), None);
        assert_eq!(layout.neighbor(2, Dir::Down), None);
        // 斜对角不算邻居。
        assert_eq!(layout.neighbor(1, Dir::Up), None);
    }
}
