//! 终端要连什么（[`TerminalTarget`]）与连上之后是谁在搬字节（[`TerminalBackend`]）。
//!
//! 两条路的差别不只是「数据从哪来」，还是**谁在驱动终端模拟器**：
//!
//! - **本地 PTY**：alacritty 的 `EventLoop` 拥有 term 并自己把 PTY 输出写进去，我们只是
//!   通过 [`PtySender`] 往 PTY 里塞按键、改尺寸；另外有真实子进程可采样
//!   （pid / 前台进程名 / cwd）。
//! - **远端 SSH**：russh 只给字节，没有本地 PTY、没有本地子进程，也没人替我们喂 term ⇒
//!   必须由 `Terminal` 自己把远端字节 `advance` 进模拟器（见 `Terminal::process_ssh_event`），
//!   于是 pid / cwd 这一类「本地进程」信息全部为 `None`。
//!
//! 所以这里抽象的是**三件必须分派的事**（写输入、改尺寸、关连接）与
//! **一件必须承认缺失的事**（本地进程信息）。

use std::borrow::Cow;
use std::sync::Arc;

use ssh::{SshFs, SshParams, SshSession, SshSize};
use util::shell::Shell;

use crate::alacritty::PtySender;
use crate::pty_info::PtyProcessInfo;
use crate::TerminalBounds;

/// 终端要连的目标。
#[derive(Clone, Debug)]
pub enum TerminalTarget {
    /// 本地：起一个 shell 进程，走 PTY（默认）。
    Local(Shell),
    /// 远端：用 russh 直连该主机，不创建任何本地进程。
    Ssh(SshParams),
}

impl TerminalTarget {
    /// 本地系统 shell —— 最常用的一种。
    pub fn system_shell() -> Self {
        Self::Local(Shell::System)
    }

    /// 是不是远端连接（`Terminal` 据此决定要不要走 SSH 那套收尾逻辑）。
    pub fn is_ssh(&self) -> bool {
        matches!(self, Self::Ssh(_))
    }
}

impl Default for TerminalTarget {
    fn default() -> Self {
        Self::system_shell()
    }
}

/// 已经建立的后端。
pub(crate) enum TerminalBackend {
    /// 本地 PTY：写读都经 alacritty 的事件循环。
    Pty {
        /// 往 PTY 写输入 / 改尺寸 / 关掉事件循环的句柄。
        pty_tx: PtySender,
        /// 本地子进程信息（pid / 前台进程 / cwd），由 `pty_info` 后台采样。
        info: Arc<PtyProcessInfo>,
    },
    /// 远端 SSH：字节直接进出信道（见模块文档）。
    Ssh(SshSession),
}

impl TerminalBackend {
    /// 把输入（键盘 / 粘贴）送出去。
    pub(crate) fn write(&self, input: impl Into<Cow<'static, [u8]>>) {
        match self {
            Self::Pty { pty_tx, .. } => pty_tx.notify(input),
            Self::Ssh(session) => session.write(&input.into()),
        }
    }

    /// 通知对端窗口尺寸变了。
    pub(crate) fn resize(&self, bounds: TerminalBounds) {
        match self {
            Self::Pty { pty_tx, .. } => pty_tx.resize(bounds),
            Self::Ssh(session) => session.resize(ssh_size_from_bounds(bounds)),
        }
    }

    /// 关掉后端（本地会话退出 / 远端断开）。
    pub(crate) fn shutdown(&self) {
        match self {
            Self::Pty { pty_tx, .. } => pty_tx.shutdown(),
            Self::Ssh(session) => session.disconnect(),
        }
    }

    /// 本地子进程信息；**远端没有**（SSH 是纯网络连接）。
    pub(crate) fn local_process_info(&self) -> Option<&Arc<PtyProcessInfo>> {
        match self {
            Self::Pty { info, .. } => Some(info),
            Self::Ssh(_) => None,
        }
    }

    /// 远端文件系统句柄；**本地没有**（本地直接用 `std::fs`）。
    ///
    /// 见 `ssh` crate 的 `fs` 模块：它是**懒连接**，拿句柄不会连网。
    pub(crate) fn remote_fs(&self) -> Option<SshFs> {
        match self {
            Self::Pty { .. } => None,
            Self::Ssh(session) => Some(session.fs()),
        }
    }
}

/// 建 SSH 会话时申请 PTY 的**初始**尺寸。
///
/// 刻意不用 [`TerminalBounds::default()`]（100×6 的调试值）：远端 shell / readline 看到 6 行
/// 会重排甚至清屏，而真实尺寸要等第一帧布局才报上去。80×24 是终端界的通用初值；这段时间里
/// 远端可能已经发来的输出由 `Terminal::feed_remote_bytes` 先攒着，等首次重排后补写。
pub(crate) const INITIAL_PTY_SIZE: SshSize = SshSize {
    cols: 80,
    rows: 24,
    pixel_width: 0,
    pixel_height: 0,
};

/// 终端尺寸 → SSH 的 PTY 尺寸（行列 + 像素）。
///
/// 行列是远端真正用来排版的值；像素只给少数图形化程序看，所以这里按单元格尺寸算出来
/// 一起上报（算不出来时也不会是 0 —— 单元格尺寸在首帧就有）。
pub(crate) fn ssh_size_from_bounds(bounds: TerminalBounds) -> SshSize {
    let cols = bounds.num_columns() as u32;
    let rows = bounds.num_lines() as u32;
    SshSize {
        cols,
        rows,
        pixel_width: (cols as f32 * f32::from(bounds.cell_width())).round() as u32,
        pixel_height: (rows as f32 * f32::from(bounds.line_height())).round() as u32,
    }
}
