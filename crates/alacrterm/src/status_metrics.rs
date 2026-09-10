//! 状态栏指标采样：会话连接状态 + 会话进程的 CPU / 内存 + 系统网络速率。
//!
//! 采样由 [`AppRoot::start_metrics_sampling`] 在后台定时任务里驱动（每
//! [`SAMPLE_INTERVAL`] 一次），结果写进普通字段 [`SystemMonitor::metrics`]，
//! 渲染时直接读取 [`SystemMonitor::metrics`]，无需再动实体。
//!
//! ## 数据来源与边界
//! - **CPU / 内存**：取自「当前会话对应的那个进程」（PTY 里的 shell / ssh），
//!   PID 由 [`terminal_view::TerminalView::pid`] 提供。
//!   sysinfo 要求两次刷新间隔不小于 `MINIMUM_CPU_UPDATE_INTERVAL`（200ms），
//!   1.5s 的采样间隔满足要求；首次采样拿不到有效的 CPU 百分比（需要两次采样的差值），
//!   表现是启动后 1.5s 内显示 `--`。
//! - **网络**：是**系统整体**的收发速率。按进程统计网络流量需要平台特定 API
//!   （如 Windows 的 ETW），sysinfo 不提供，因此这里只能给出系统总量；
//!   想知道「这个会话占了多少带宽」需要另接平台接口。

use std::time::{Duration, Instant};

use sysinfo::{Networks, Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// 采样间隔，同时也是 CPU 百分比的计算窗口。
pub(crate) const SAMPLE_INTERVAL: Duration = Duration::from_millis(1500);

/// 一次采样的结果（渲染只读这里的值）。
#[derive(Clone, Copy, Default)]
pub(crate) struct SessionMetrics {
    /// 会话进程是否仍在运行。
    ///
    /// `None` = 还没采样到（启动初期 / PTY 未就绪），`Some(true)` = 运行中，
    /// `Some(false)` = 进程已从进程表消失（已结束）。三态是必要的：如果只用 bool，
    /// 刚启动的那 1.5s（首次采样前）会被误显示为「已断开」。
    pub(crate) alive: Option<bool>,
    /// 会话进程的 CPU 占用百分比（多核可超过 100）。
    pub(crate) cpu_percent: Option<f32>,
    /// 会话进程的常驻内存（字节）。
    pub(crate) memory_bytes: Option<u64>,
    /// 系统网络接收速率（字节/秒）。
    pub(crate) net_rx_per_sec: f64,
    /// 系统网络发送速率（字节/秒）。
    pub(crate) net_tx_per_sec: f64,
}

/// 采样器：持有 sysinfo 的句柄，以及上一次采样用于求差值的状态。
pub(crate) struct SystemMonitor {
    system: System,
    networks: Networks,
    /// 上一次采样时刻（把网络累计流量换算成速率用）。
    last_sample: Option<Instant>,
    /// 上一次采样时的网络累计（接收, 发送）字节数。
    last_net_totals: Option<(u64, u64)>,
    metrics: SessionMetrics,
}

impl Default for SystemMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemMonitor {
    pub(crate) fn new() -> Self {
        Self {
            // 进程表按需刷新（在 `sample` 里按 PID 精确刷新），初始为空即可。
            system: System::new(),
            networks: Networks::new_with_refreshed_list(),
            last_sample: None,
            last_net_totals: None,
            metrics: SessionMetrics::default(),
        }
    }

    /// 最近一次采样的结果。
    pub(crate) fn metrics(&self) -> SessionMetrics {
        self.metrics
    }

    /// 采样一次。`pid` 是当前会话进程的 PID；无会话或 PTY 未就绪时传 `None`。
    pub(crate) fn sample(&mut self, pid: Option<u32>) {
        let now = Instant::now();
        self.sample_process(pid);
        self.sample_network(now);
    }

    /// 会话进程：存活状态 + CPU + 内存。
    fn sample_process(&mut self, pid: Option<u32>) {
        // 每轮先清空：PID 拿不到就是「未知」，进程消失则是「已结束」。
        self.metrics.alive = None;
        self.metrics.cpu_percent = None;
        self.metrics.memory_bytes = None;

        let Some(pid) = pid else {
            return;
        };
        let pid = Pid::from_u32(pid);

        let refresh_kind = ProcessRefreshKind::nothing().with_cpu().with_memory();
        // remove_dead_processes = true：进程退出后把它从进程表里移除，
        // 这样再采样时会返回 0 而不是继续报旧数据。
        let refreshed = self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            refresh_kind,
        );
        if refreshed == 0 {
            // 有 PID 却刷不到进程 → 已经结束。
            self.metrics.alive = Some(false);
            return;
        }

        if let Some(process) = self.system.process(pid) {
            self.metrics.alive = Some(true);
            self.metrics.cpu_percent = Some(process.cpu_usage());
            self.metrics.memory_bytes = Some(process.memory());
        }
    }

    /// 系统网络：用累计值差分求出每秒收发速率。
    fn sample_network(&mut self, now: Instant) {
        self.networks.refresh(true);
        let totals = self.networks.list().values().fold((0u64, 0u64), |acc, data| {
            (
                acc.0.saturating_add(data.received()),
                acc.1.saturating_add(data.transmitted()),
            )
        });

        if let (Some((prev_rx, prev_tx)), Some(last)) = (self.last_net_totals, self.last_sample) {
            let seconds = now.duration_since(last).as_secs_f64();
            if seconds > 0.0 {
                // saturating_sub：网卡重置 / 计数回绕时避免出现负数速率。
                self.metrics.net_rx_per_sec =
                    totals.0.saturating_sub(prev_rx) as f64 / seconds;
                self.metrics.net_tx_per_sec =
                    totals.1.saturating_sub(prev_tx) as f64 / seconds;
            }
        }

        self.last_net_totals = Some(totals);
        self.last_sample = Some(now);
    }
}

/// 字节数 → 便于阅读的字符串（B / KB / MB / GB）。
pub(crate) fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let value = bytes as f64;
    if value >= GB {
        format!("{:.1} GB", value / GB)
    } else if value >= MB {
        format!("{:.0} MB", value / MB)
    } else if value >= KB {
        format!("{:.0} KB", value / KB)
    } else {
        format!("{bytes} B")
    }
}

/// 速率 → 便于阅读的字符串（自动补 `/s`；无流量时给 `0`）。
pub(crate) fn format_rate(bytes_per_sec: f64) -> String {
    if bytes_per_sec < 1.0 {
        return "0".to_string();
    }
    format!("{}/s", format_bytes(bytes_per_sec as u64))
}
