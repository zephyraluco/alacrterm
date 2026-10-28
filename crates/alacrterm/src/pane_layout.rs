//! 分屏布局：一个标签页内部的**格子树**（kitty / ghostty 风格）。
//!
//! 每次分屏把一片叶子换成 `Split`（二叉），关格时摘掉叶子并把「只剩一个孩子」的
//! 分叉折叠掉。分叉记两样东西：`ratio`（`first` 占的份额，拖分隔条改它，见
//! [`ratio_at`]）与 `node`（分叉的稳定 id —— 元素 id 与拖拽载荷都靠它认分叉）。
//!
//! 叶子存的是**下标**（进 [`crate::terminal_panel::TabPane::panes`]），所以摘掉一片后
//! 要把比它大的下标全部前移一位。
//!
//! 渲染不在这里：分叉要挂拖拽监听、两侧按 `ratio` 给 `flex_basis`，都在
//! [`crate::terminal_panel::TabPane::render_layout`] 里做。

use gpui::Axis;

/// 拖拽分隔条时每一侧至少留的宽度（像素）；容器装不下时退化成对半分。
pub(crate) const MIN_PANE: f32 = 80.;

/// 指针位置 → 分叉比例：`position` 落在 `[origin, origin + extent]` 里的相对位置。
pub(crate) fn ratio_at(origin: f32, extent: f32, position: f32) -> f32 {
    if extent <= 0. {
        // 还没布局（或容器塌了）：别除零，先给对半分。
        return 0.5;
    }
    let edge = (MIN_PANE / extent).min(0.5);
    ((position - origin) / extent).clamp(edge, 1. - edge)
}

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
    /// 一次二分割：`first` 在左 / 上（占 `ratio` 那一份），`second` 在右 / 下。
    Split {
        /// 稳定 id（分隔条的元素 id 与拖拽载荷都用它）。
        node: u64,
        axis: Axis,
        /// `first` 占的份额。
        ratio: f32,
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
    /// ——新格子落在目标格子的**右 / 下**，`node` 是新分叉的 id。
    ///
    /// 返回 `false` 表示树里没有 `target`（调用方不应该把新格子登记进面板表）。
    pub(crate) fn split(&mut self, target: usize, axis: Axis, new_ix: usize, node: u64) -> bool {
        let Some(slot) = leaf_mut(self, target) else {
            return false;
        };
        let old = std::mem::replace(slot, Layout::Leaf(usize::MAX));
        *slot = Layout::Split {
            node,
            axis,
            ratio: 0.5,
            first: Box::new(old),
            second: Box::new(Layout::Leaf(new_ix)),
        };
        true
    }

    /// 改某个分叉的份额；返回 `false` 表示没这个分叉（或比例没变）。
    pub(crate) fn set_ratio(&mut self, node: u64, ratio: f32) -> bool {
        match split_mut(self, node) {
            Some(Layout::Split { ratio: slot, .. }) if *slot != ratio => {
                *slot = ratio;
                true
            }
            _ => false,
        }
    }

    /// 某个分叉的分割轴（拖拽时用它决定读指针的哪个分量）。
    pub(crate) fn axis_of(&self, node: u64) -> Option<Axis> {
        match split_ref(self, node) {
            Some(Layout::Split { axis, .. }) => Some(*axis),
            _ => None,
        }
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
}

/// 找到并借出「就是 `Leaf(ix)`」的那个节点。
fn leaf_mut(layout: &mut Layout, ix: usize) -> Option<&mut Layout> {
    if matches!(layout, Layout::Leaf(i) if *i == ix) {
        return Some(layout);
    }
    match layout {
        Layout::Leaf(_) => None,
        Layout::Split { first, second, .. } => leaf_mut(first, ix).or_else(|| leaf_mut(second, ix)),
    }
}

/// 找到 id 为 `node` 的分叉。
fn split_ref(layout: &Layout, node: u64) -> Option<&Layout> {
    match layout {
        Layout::Leaf(_) => None,
        Layout::Split {
            node: id,
            first,
            second,
            ..
        } => {
            if *id == node {
                return Some(layout);
            }
            split_ref(first, node).or_else(|| split_ref(second, node))
        }
    }
}

/// 借出 id 为 `node` 的分叉。
fn split_mut(layout: &mut Layout, node: u64) -> Option<&mut Layout> {
    if matches!(layout, Layout::Split { node: id, .. } if *id == node) {
        return Some(layout);
    }
    match layout {
        Layout::Leaf(_) => None,
        Layout::Split { first, second, .. } => {
            split_mut(first, node).or_else(|| split_mut(second, node))
        }
    }
}

/// 摘掉叶子 `ix`；`None` = 这棵子树整个没了，`Some` = 剩下的部分（只剩一边时直接顶上来）。
fn pruned(layout: Layout, ix: usize) -> Option<Layout> {
    match layout {
        Layout::Leaf(i) if i == ix => None,
        Layout::Leaf(i) => Some(Layout::Leaf(i)),
        Layout::Split {
            node,
            axis,
            ratio,
            first,
            second,
        } => match (pruned(*first, ix), pruned(*second, ix)) {
            (Some(first), Some(second)) => Some(Layout::Split {
                node,
                axis,
                ratio,
                first: Box::new(first),
                second: Box::new(second),
            }),
            // 只剩一边：这个分叉没用了，剩下的直接顶上来（`node` 也一起没了）。
            (Some(only), None) | (None, Some(only)) => Some(only),
            (None, None) => None,
        },
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
        // 按真实比例摊（拖过分隔条之后 50/50 的近似会挑错邻居）。
        Layout::Split {
            axis,
            ratio,
            first,
            second,
            ..
        } => match axis {
            Axis::Horizontal => {
                let w = rect.w * ratio;
                collect_rects(first, UnitRect { w, ..rect }, out);
                collect_rects(
                    second,
                    UnitRect {
                        x: rect.x + w,
                        w: rect.w - w,
                        ..rect
                    },
                    out,
                );
            }
            Axis::Vertical => {
                let h = rect.h * ratio;
                collect_rects(first, UnitRect { h, ..rect }, out);
                collect_rects(
                    second,
                    UnitRect {
                        y: rect.y + h,
                        h: rect.h - h,
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

    fn split(node: u64, axis: Axis, first: Layout, second: Layout) -> Layout {
        Layout::Split {
            node,
            axis,
            ratio: 0.5,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    /// 浮点比较（比例是算出来的，别用 `==`）。
    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn split_puts_the_new_pane_after_the_target() {
        let mut layout = Layout::single();
        assert!(layout.split(0, Axis::Horizontal, 1, 7));
        assert_eq!(layout, split(7, Axis::Horizontal, leaf(0), leaf(1)));
    }

    #[test]
    fn split_missing_target_is_a_no_op() {
        let mut layout = Layout::single();
        assert!(!layout.split(3, Axis::Horizontal, 1, 7));
        assert_eq!(layout, Layout::single());
    }

    #[test]
    fn split_targets_a_nested_leaf() {
        let mut layout = split(1, Axis::Horizontal, leaf(0), leaf(1));
        layout.split(1, Axis::Vertical, 2, 9);
        assert_eq!(
            layout,
            split(
                1,
                Axis::Horizontal,
                leaf(0),
                split(9, Axis::Vertical, leaf(1), leaf(2))
            )
        );
    }

    #[test]
    fn removing_a_leaf_collapses_its_parent() {
        let mut layout = split(1, Axis::Horizontal, leaf(0), leaf(1));
        layout.remove(0);
        assert_eq!(layout, leaf(0));
    }

    #[test]
    fn removing_renumbers_the_leaves_after_it() {
        let mut layout = split(
            1,
            Axis::Horizontal,
            leaf(0),
            split(2, Axis::Vertical, leaf(1), leaf(2)),
        );
        layout.remove(1);
        // 原来的 2 号前移成 1 号，左边的 0 号不动。
        assert_eq!(layout, split(1, Axis::Horizontal, leaf(0), leaf(1)));
    }

    #[test]
    fn removing_the_last_leaf_keeps_the_tree_prefix() {
        let mut layout = split(
            1,
            Axis::Horizontal,
            leaf(0),
            split(2, Axis::Vertical, leaf(1), leaf(2)),
        );
        layout.remove(2);
        assert_eq!(layout, split(1, Axis::Horizontal, leaf(0), leaf(1)));
    }

    #[test]
    fn set_ratio_targets_only_its_own_node() {
        let mut layout = split(
            1,
            Axis::Horizontal,
            leaf(0),
            split(2, Axis::Vertical, leaf(1), leaf(2)),
        );
        assert!(layout.set_ratio(2, 0.3));
        assert_eq!(layout.axis_of(2), Some(Axis::Vertical));
        assert_eq!(layout.axis_of(1), Some(Axis::Horizontal));
        assert_eq!(layout.axis_of(9), None);
        // 比例没变 / 没这个分叉都不算改动。
        assert!(!layout.set_ratio(2, 0.3));
        assert!(!layout.set_ratio(9, 0.3));
    }

    #[test]
    fn ratio_keeps_a_minimum_pane_on_both_sides() {
        // 容器 1000px：两侧各至少 80px（8%）。
        assert!(close(ratio_at(0., 1000., 500.), 0.5));
        assert!(close(ratio_at(0., 1000., -200.), 0.08));
        assert!(close(ratio_at(0., 1000., 1200.), 0.92));
        // 带原点（嵌套分叉时容器不从 0 开始）。
        assert!(close(ratio_at(100., 1000., 600.), 0.5));
        // 容器装不下两侧的下限：退化成对半分，而不是把某一侧压没。
        assert!(close(ratio_at(0., 100., 0.), 0.5));
        // 还没布局：不除零。
        assert!(close(ratio_at(0., 0., 10.), 0.5));
    }

    #[test]
    fn neighbor_walks_the_axes() {
        //  ┌───────┬───────┐
        //  │   0   │   1   │
        //  │       ├───────┤
        //  │       │   2   │
        //  └───────┴───────┘
        let layout = split(
            1,
            Axis::Horizontal,
            leaf(0),
            split(2, Axis::Vertical, leaf(1), leaf(2)),
        );
        assert_eq!(layout.neighbor(0, Dir::Right), Some(1));
        assert_eq!(layout.neighbor(1, Dir::Left), Some(0));
        assert_eq!(layout.neighbor(1, Dir::Down), Some(2));
        assert_eq!(layout.neighbor(2, Dir::Up), Some(1));
        assert_eq!(layout.neighbor(0, Dir::Left), None);
        assert_eq!(layout.neighbor(2, Dir::Down), None);
        // 斜对角不算邻居。
        assert_eq!(layout.neighbor(1, Dir::Up), None);
    }

    #[test]
    fn neighbor_follows_the_dragged_ratio() {
        // 左边很宽（占比 0.9）：右上 / 右下两格都缩在右侧窄条里，几何必须按比例算。
        let mut layout = split(
            1,
            Axis::Horizontal,
            leaf(0),
            split(2, Axis::Vertical, leaf(1), leaf(2)),
        );
        assert!(layout.set_ratio(1, 0.9));
        assert_eq!(layout.neighbor(0, Dir::Right), Some(1));
        assert_eq!(layout.neighbor(1, Dir::Left), Some(0));
        assert_eq!(layout.neighbor(1, Dir::Down), Some(2));
        assert_eq!(layout.neighbor(2, Dir::Up), Some(1));
    }
}
