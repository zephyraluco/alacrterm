use gpui::{Context, Task};
use parking_lot::{MappedRwLockReadGuard, Mutex, RwLock, RwLockReadGuard};
use std::{path::PathBuf, sync::Arc};

#[cfg(target_os = "windows")]
use windows::Win32::{Foundation::HANDLE, System::Threading::GetProcessId};

use sysinfo::{Pid, Process, ProcessRefreshKind, RefreshKind, System, UpdateKind};

use crate::{Event, Terminal};

#[derive(Clone, Copy)]
pub struct ProcessIdGetter(PidSource);

/// 会话进程信息的来源。
#[derive(Clone, Copy)]
enum PidSource {
    /// 本地 PTY：子进程 pid 已知，前台进程组从 PTY 句柄查询。
    Pty { handle: i32, fallback_pid: u32 },
    /// 没有本地子进程（SSH 远端会话）。
    ///
    /// 远端的 shell 不在本机进程表里，任何「本机进程」都是无关的，因此一律报告 `None`：
    /// 状态栏的 CPU / 内存显示 `--`，标题回退到会话名，也不会误杀无关进程。
    None,
}

impl ProcessIdGetter {
    pub(crate) fn new(handle: i32, fallback_pid: u32) -> ProcessIdGetter {
        ProcessIdGetter(PidSource::Pty {
            handle,
            fallback_pid,
        })
    }

    /// 用于 SSH 等没有本地子进程的后端。
    pub(crate) fn none() -> ProcessIdGetter {
        ProcessIdGetter(PidSource::None)
    }

    pub fn fallback_pid(&self) -> Pid {
        match self.0 {
            PidSource::Pty { fallback_pid, .. } => Pid::from_u32(fallback_pid),
            PidSource::None => Pid::from_u32(0),
        }
    }

    /// 是否指向本机上真实存在的子进程（`PtySource::None` 时为假）。
    ///
    /// [`PtyProcessInfo::terminate_child_process`] 用它避免把 pid 0 送进
    /// `killpg(0, ..)`（那会作用到**本进程所在的整个进程组**）。
    #[cfg(unix)]
    fn is_local(&self) -> bool {
        matches!(self.0, PidSource::Pty { .. })
    }
}

#[cfg(unix)]
impl ProcessIdGetter {
    fn pid(&self) -> Option<Pid> {
        let PidSource::Pty {
            handle,
            fallback_pid,
        } = self.0
        else {
            return None;
        };
        // Negative pid means error.
        // Zero pid means no foreground process group is set on the PTY yet.
        // Avoid killing the current process by returning a zero pid.
        let pid = unsafe { libc::tcgetpgrp(handle) };
        if pid > 0 {
            return Some(Pid::from_u32(pid as u32));
        }

        if fallback_pid > 0 {
            return Some(Pid::from_u32(fallback_pid));
        }

        None
    }
}

#[cfg(windows)]
impl ProcessIdGetter {
    fn pid(&self) -> Option<Pid> {
        let PidSource::Pty {
            handle,
            fallback_pid,
        } = self.0
        else {
            return None;
        };
        let pid = unsafe { GetProcessId(HANDLE(handle as _)) };
        // the GetProcessId may fail and returns zero, which will lead to a stack overflow issue
        if pid == 0 {
            // in the builder process, there is a small chance, almost negligible,
            // that this value could be zero, which means child_watcher returns None,
            // GetProcessId returns 0.
            if fallback_pid == 0 {
                return None;
            }
            return Some(Pid::from_u32(fallback_pid));
        }
        Some(Pid::from_u32(pid))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ProcessInfo {
    pub(crate) name: String,
    pub(crate) cwd: PathBuf,
    pub(crate) argv: Vec<String>,
}

/// Fetches Zed-relevant Pseudo-Terminal (PTY) process information
pub(crate) struct PtyProcessInfo {
    system: RwLock<System>,
    refresh_kind: ProcessRefreshKind,
    pid_getter: ProcessIdGetter,
    pub(crate) current: RwLock<Option<ProcessInfo>>,
    task: Mutex<Option<Task<()>>>,
}

impl PtyProcessInfo {
    pub(crate) fn new(pid_getter: ProcessIdGetter) -> PtyProcessInfo {
        let process_refresh_kind = ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_cwd(UpdateKind::Always)
            .with_exe(UpdateKind::Always);
        let refresh_kind = RefreshKind::nothing().with_processes(process_refresh_kind);
        let system = System::new_with_specifics(refresh_kind);

        PtyProcessInfo {
            system: RwLock::new(system),
            refresh_kind: process_refresh_kind,
            pid_getter,
            current: RwLock::new(None),
            task: Mutex::new(None),
        }
    }

    pub(crate) fn pid_getter(&self) -> &ProcessIdGetter {
        &self.pid_getter
    }

    fn refresh(&self) -> Option<MappedRwLockReadGuard<'_, Process>> {
        let pid = self.pid_getter.pid()?;
        if self.system.write().refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[pid]),
            true,
            self.refresh_kind,
        ) == 1
        {
            RwLockReadGuard::try_map(self.system.read(), |system| system.process(pid)).ok()
        } else {
            None
        }
    }

    fn get_child(&self) -> Option<MappedRwLockReadGuard<'_, Process>> {
        let pid = self.pid_getter.fallback_pid();
        RwLockReadGuard::try_map(self.system.read(), |system| system.process(pid)).ok()
    }

    // 从 Zed 移植后本仓库暂未使用（保留备用），显式允许以免每次编译都报 dead_code。
    #[cfg(unix)]
    #[allow(dead_code)]
    pub(crate) fn kill_current_process(&self) -> bool {
        let Some(pid) = self.pid_getter.pid() else {
            return false;
        };
        unsafe { libc::killpg(pid.as_u32() as i32, libc::SIGKILL) == 0 }
    }

    #[cfg(not(unix))]
    #[allow(dead_code)]
    pub(crate) fn kill_current_process(&self) -> bool {
        self.refresh().is_some_and(|process| process.kill())
    }

    pub(crate) fn kill_child_process(&self) -> bool {
        self.get_child().is_some_and(|process| process.kill())
    }

    #[cfg(unix)]
    pub(crate) fn terminate_child_process(&self) -> bool {
        // 没有本地子进程时 `fallback_pid()` 是 0，`killpg(0, ..)` 会作用到本进程所在的
        // 整个进程组（等于把自己也杀掉），必须先挡住。
        if !self.pid_getter.is_local() {
            return false;
        }
        let pid = self.pid_getter.fallback_pid();
        unsafe { libc::killpg(pid.as_u32() as i32, libc::SIGTERM) == 0 }
    }

    #[cfg(not(unix))]
    pub(crate) fn terminate_child_process(&self) -> bool {
        false
    }

    fn load(&self) -> Option<ProcessInfo> {
        let process = self.refresh()?;
        let cwd = process.cwd().map_or(PathBuf::new(), |p| p.to_owned());

        let info = ProcessInfo {
            name: process.name().to_str()?.to_owned(),
            cwd,
            argv: process
                .cmd()
                .iter()
                .filter_map(|s| s.to_str().map(ToOwned::to_owned))
                .collect(),
        };
        *self.current.write() = Some(info.clone());
        Some(info)
    }

    #[cfg(all(test, unix))]
    pub(crate) fn load_for_test(&self) -> Option<ProcessInfo> {
        self.load()
    }

    /// Updates the cached process info, emitting a [`Event::TitleChanged`] event if the Zed-relevant info has changed
    pub(crate) fn emit_title_changed_if_changed(self: &Arc<Self>, cx: &mut Context<'_, Terminal>) {
        if self.task.lock().is_some() {
            return;
        }
        let this = self.clone();
        let has_changed = cx.background_executor().spawn(async move {
            let previous = this.current.read().clone();
            let current = this.load();
            let has_changed = match (previous.as_ref(), current.as_ref()) {
                (None, None) => false,
                (Some(prev), Some(now)) => prev.cwd != now.cwd || prev.name != now.name,
                _ => true,
            };
            if has_changed {
                *this.current.write() = current;
            }
            has_changed
        });
        let this = Arc::downgrade(self);
        *self.task.lock() = Some(cx.spawn(async move |term, cx| {
            if has_changed.await {
                term.update(cx, |_, cx| cx.emit(Event::TitleChanged)).ok();
            }
            if let Some(this) = this.upgrade() {
                this.task.lock().take();
            }
        }));
    }

    pub(crate) fn pid(&self) -> Option<Pid> {
        self.pid_getter.pid()
    }
}
