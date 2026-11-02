//! Background system sampling for the active terminal's compact status strip.

use std::{
    collections::{BTreeMap, VecDeque},
    time::{Duration, Instant},
};

use futures::{FutureExt as _, StreamExt as _, channel::mpsc, future::Either};
use gpui::{
    App, Context, EntityId, IntoElement, ParentElement as _, Render, Styled as _, Task, Window,
    div, px,
};
use gpui_kit::component::{
    ActiveTheme as _, Icon, Placement, Sizable as _,
    button::{Button, ButtonVariants as _},
    chart::LineChart,
    h_flex,
};

use crate::assets::IconName;

const REMOTE_TIMEOUT: Duration = Duration::from_secs(12);
const FRAME_END: &str = "__ALACRTERM_STATUS_END__\n";
const REMOTE_SCRIPT: &str = include_str!("../../../scripts/system_status.sh");
type Traffic = BTreeMap<String, (u64, u64)>;

#[derive(Clone, Default, Debug)]
struct Snapshot {
    host: String,
    user: String,
    cores: usize,
    sessions: Option<usize>,
    cpu: Option<f32>,
    memory_used: u64,
    memory_total: u64,
    upload: Option<f64>,
    download: Option<f64>,
    uptime: u64,
    disk: Option<(String, u64, u64)>,
}

#[derive(Default)]
struct Rates {
    previous: Option<(Instant, Traffic)>,
}
impl Rates {
    fn update(&mut self, now: Instant, traffic: Traffic) -> (Option<f64>, Option<f64>) {
        let Some((before, previous)) = self.previous.replace((now, traffic.clone())) else {
            return (None, None);
        };
        let elapsed = now.duration_since(before).as_secs_f64();
        if elapsed == 0.0 {
            return (None, None);
        }
        let (mut received, mut transmitted) = (0_u64, 0_u64);
        for (name, (rx, tx)) in traffic {
            if let Some(&(old_rx, old_tx)) = previous.get(&name) {
                // A reset or a newly discovered interface is a new baseline, not a burst.
                received = received.saturating_add(rx.saturating_sub(old_rx));
                transmitted = transmitted.saturating_add(tx.saturating_sub(old_tx));
            }
        }
        (
            Some(received as f64 / elapsed),
            Some(transmitted as f64 / elapsed),
        )
    }
}

fn is_loopback(name: &str) -> bool {
    name == "lo" || name.starts_with("lo0") || name.to_ascii_lowercase().contains("loopback")
}

#[derive(Default)]
struct RemoteSampler {
    cpu: Option<(u64, u64)>,
    rates: Rates,
}
impl RemoteSampler {
    fn parse(&mut self, frame: &str) -> anyhow::Result<Snapshot> {
        let mut snapshot = Snapshot::default();
        let (mut total, mut idle) = (None, None);
        let (mut free, mut available, mut cached, mut buffers, mut reclaimable, mut shared) =
            (0, None, 0, 0, 0, 0);
        let mut traffic = Traffic::new();
        let mut network_available = false;
        let mut uptime_seen = false;
        for line in frame.lines() {
            let Some((key, value)) = line.split_once(' ') else {
                continue;
            };
            match key {
                "host" => snapshot.host = value.trim().into(),
                "user" => snapshot.user = value.trim().into(),
                "cpu" => {
                    let ticks = value
                        .split_whitespace()
                        .take(8)
                        .map(str::parse::<u64>)
                        .collect::<Result<Vec<_>, _>>()?;
                    anyhow::ensure!(ticks.len() >= 4, "无效的 CPU 数据");
                    total = Some(ticks.iter().copied().fold(0, u64::saturating_add));
                    idle = Some(ticks[3].saturating_add(ticks.get(4).copied().unwrap_or(0)));
                }
                "cores" => snapshot.cores = value.trim().parse()?,
                "sessions" => snapshot.sessions = Some(value.trim().parse()?),
                "mem" => {
                    let fields: Vec<_> = value.split_whitespace().collect();
                    if fields.len() < 2 {
                        continue;
                    }
                    let bytes = fields[1].parse::<u64>()?.saturating_mul(1024);
                    match fields[0] {
                        "MemTotal:" => snapshot.memory_total = bytes,
                        "MemAvailable:" => available = Some(bytes),
                        "MemFree:" => free = bytes,
                        "Buffers:" => buffers = bytes,
                        "Cached:" => cached = bytes,
                        "SReclaimable:" => reclaimable = bytes,
                        "Shmem:" => shared = bytes,
                        _ => {}
                    }
                }
                "net" => {
                    let Some((name, values)) = value.split_once(':') else {
                        continue;
                    };
                    let fields: Vec<_> = values.split_whitespace().collect();
                    if fields.len() >= 9 {
                        network_available = true;
                        if !is_loopback(name.trim()) {
                            traffic.insert(
                                name.trim().into(),
                                (fields[0].parse()?, fields[8].parse()?),
                            );
                        }
                    }
                }
                "uptime" => {
                    snapshot.uptime = value
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .parse::<f64>()? as u64;
                    uptime_seen = true;
                }
                "disk" => {
                    let fields: Vec<_> = value.split_whitespace().collect();
                    if fields.len() >= 6 {
                        snapshot.disk = Some((
                            "/".into(),
                            fields[2].parse::<u64>()?.saturating_mul(1024),
                            fields[1].parse::<u64>()?.saturating_mul(1024),
                        ));
                    }
                }
                _ => {}
            }
        }
        anyhow::ensure!(
            !snapshot.host.is_empty() && snapshot.memory_total > 0 && uptime_seen,
            "远端未提供有效的 Linux 系统数据"
        );
        let (total, idle) = (
            total.ok_or_else(|| anyhow::anyhow!("缺少 CPU 数据"))?,
            idle.unwrap_or(0),
        );
        if let Some((old_total, old_idle)) = self.cpu.replace((total, idle)) {
            let elapsed = total.saturating_sub(old_total);
            if elapsed > 0 {
                snapshot.cpu = Some(
                    (100.0 * (elapsed.saturating_sub(idle.saturating_sub(old_idle))) as f64
                        / elapsed as f64) as f32,
                );
            }
        }
        snapshot.memory_used = snapshot
            .memory_total
            .saturating_sub(available.unwrap_or_else(|| {
                free.saturating_add(buffers)
                    .saturating_add(cached)
                    .saturating_add(reclaimable)
                    .saturating_sub(shared)
            }));
        if network_available {
            (snapshot.download, snapshot.upload) = self.rates.update(Instant::now(), traffic);
        } else {
            self.rates = Rates::default();
        }
        Ok(snapshot)
    }
}

pub(crate) struct SystemMonitor {
    target: Option<(EntityId, ssh::SessionId)>,
    remote: bool,
    snapshot: Option<Snapshot>,
    message: String,
    task: Option<Task<()>>,
    client: Option<ssh::SshExec>,
    host: String,
    history: VecDeque<f32>,
    failed: bool,
}
impl SystemMonitor {
    pub(crate) fn new(_: &mut Context<Self>) -> Self {
        Self {
            target: None,
            remote: false,
            snapshot: None,
            message: String::new(),
            task: None,
            client: None,
            host: String::new(),
            history: VecDeque::new(),
            failed: false,
        }
    }

    pub(crate) fn set_target(
        &mut self,
        target: Option<((EntityId, ssh::SessionId), ssh::SshExec, String)>,
        remote: bool,
        cx: &mut Context<Self>,
    ) {
        let id = target.as_ref().map(|(id, _, _)| *id);
        if self.target == id && self.remote == remote {
            return;
        }
        self.target = id;
        self.remote = remote;
        self.task = None;
        self.snapshot = None;
        self.history.clear();
        self.failed = false;
        self.client = None;
        self.host.clear();
        self.message = if remote {
            "等待远端系统状态…"
        } else {
            ""
        }
        .into();
        match target {
            Some((_, client, host)) => {
                self.client = Some(client.clone());
                self.host = host;
                self.message = format!("{} · 正在采样…", self.host);
                self.start_remote(client, cx);
            }
            _ if !remote => {}
            _ => self.message = "SSH 未连接 · 系统监控暂停".into(),
        }
        cx.notify();
    }

    fn start_remote(&mut self, client: ssh::SshExec, cx: &mut Context<Self>) {
        self.task = Some(cx.spawn(async move |this, cx| {
            let (sender, mut events) = mpsc::unbounded();
            // A dedicated non-PTY channel never writes probes into the user's shell.
            let channel = match client.exec(REMOTE_SCRIPT, sender) {
                Ok(channel) => channel,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| this.fail(&error.to_string(), cx));
                    return;
                }
            };
            let mut sampler = RemoteSampler::default();
            let mut pending = Vec::new();
            let mut deadline = Instant::now() + REMOTE_TIMEOUT;
            loop {
                let timeout = cx
                    .background_executor()
                    .timer(deadline.saturating_duration_since(Instant::now()));
                let event =
                    match futures::future::select(events.next().boxed(), timeout.boxed()).await {
                        Either::Left((event, _)) => event,
                        Either::Right(_) => {
                            let _ = this.update(cx, |this, cx| this.fail("远端采样超时", cx));
                            break;
                        }
                    };
                match event {
                    Some(ssh::SshEvent::Data(data)) => {
                        pending.extend_from_slice(&data);
                        if pending.len() > 128 * 1024 {
                            let _ = this.update(cx, |this, cx| this.fail("远端数据超出限制", cx));
                            break;
                        }
                        while let Some(end) = pending
                            .windows(FRAME_END.len())
                            .position(|part| part == FRAME_END.as_bytes())
                        {
                            let frame = String::from_utf8_lossy(&pending[..end]);
                            let result = sampler.parse(&frame);
                            pending.drain(..end + FRAME_END.len());
                            deadline = Instant::now() + REMOTE_TIMEOUT;
                            if this
                                .update(cx, |this, cx| match result {
                                    Ok(snapshot) => {
                                        if let Some(cpu) = snapshot.cpu {
                                            if this.history.len() == 24 {
                                                this.history.pop_front();
                                            }
                                            this.history.push_back(cpu);
                                        }
                                        this.failed = false;
                                        this.snapshot = Some(snapshot);
                                        cx.notify();
                                    }
                                    Err(error) => this.fail(&error.to_string(), cx),
                                })
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                    Some(ssh::SshEvent::Closed {
                        reason,
                        exit_status,
                    }) => {
                        let _ = this.update(cx, |this, cx| {
                            this.fail(
                                reason.as_deref().unwrap_or(if exit_status == Some(78) {
                                    "远端不支持系统采样（需要 Linux /proc）"
                                } else {
                                    "远端采样已结束，请重试"
                                }),
                                cx,
                            )
                        });
                        break;
                    }
                    None => {
                        let _ = this.update(cx, |this, cx| this.fail("远端连接已关闭", cx));
                        break;
                    }
                    _ => {}
                }
            }
            drop(channel);
        }));
    }

    fn fail(&mut self, message: &str, cx: &mut Context<Self>) {
        self.snapshot = None;
        self.failed = true;
        self.message = format!("系统监控不可用 · {message}");
        cx.notify();
    }
}

fn bytes(value: f64) -> String {
    let (scaled, unit) = if value >= 1024_f64.powi(3) {
        (value / 1024_f64.powi(3), "GiB")
    } else if value >= 1024_f64.powi(2) {
        (value / 1024_f64.powi(2), "MiB")
    } else if value >= 1024.0 {
        (value / 1024.0, "KiB")
    } else {
        (value, "B")
    };
    format!("{scaled:.1} {unit}")
}
fn rate(value: Option<f64>) -> String {
    value
        .map(|value| format!("{}/s", bytes(value)))
        .unwrap_or_else(|| "—".into())
}
fn uptime(seconds: u64) -> String {
    let minutes = seconds / 60;
    if minutes >= 1440 {
        format!("{}天 {}时", minutes / 1440, minutes % 1440 / 60)
    } else if minutes >= 60 {
        format!("{}时 {}分", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}分")
    }
}

fn item(
    id: &'static str,
    icon: IconName,
    text: String,
    tooltip: String,
    cx: &App,
) -> impl IntoElement {
    let accessibility = format!("{tooltip}: {text}");
    Button::new(id)
        .ghost()
        .small()
        .h(super::status_bar::STATUS_BAR_ITEM_HEIGHT)
        .px_1()
        .label(text)
        .icon(icon)
        .tooltip(tooltip)
        .tooltip_placement(Placement::Top)
        .accessibility_label(accessibility)
        .text_color(cx.theme().muted_foreground)
}

impl Render for SystemMonitor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut bar = h_flex()
            .w_full()
            .h(super::status_bar::STATUS_BAR_ITEM_HEIGHT)
            .min_w_0()
            .overflow_hidden()
            .items_center()
            .gap_1()
            .text_size(px(11.))
            .whitespace_nowrap();
        if let Some(data) = &self.snapshot {
            let cpu = data
                .cpu
                .map(|cpu| format!("{cpu:.0}%"))
                .unwrap_or_else(|| "—".into());
            let memory = format!(
                "{} / {}",
                bytes(data.memory_used as f64),
                bytes(data.memory_total as f64)
            );
            let cpu_history = div()
                .w(px(72.))
                .h(px(18.))
                .flex_none()
                .overflow_hidden()
                .child(
                    LineChart::new(self.history.iter().copied().enumerate())
                        .id("monitor-cpu-history")
                        .x(|(index, _)| index.to_string())
                        .y(|(_, cpu)| cpu.clamp(0.0, 100.0))
                        .natural()
                        .stroke(cx.theme().primary)
                        .y_domain(0.0, 100.0)
                        .y_padding(1.0, 1.0)
                        .point_count(24)
                        .x_axis(false)
                        .y_axis(false)
                        .grid(false)
                        .interactive(false)
                        .appear(false),
                );
            bar = bar
                .child(item(
                    "monitor-host",
                    IconName::SquareTerminal,
                    data.host.clone(),
                    "当前 SSH 远端 · Linux".into(),
                    cx,
                ))
                .child(item(
                    "monitor-cpu",
                    IconName::Cpu,
                    cpu,
                    format!("CPU 总使用率 · {} 个逻辑核心 · 每 2 秒刷新 · 曲线为最近 48 秒历史（0–100%）", data.cores),
                    cx,
                ))
                .child(cpu_history)
                .child(item(
                    "monitor-memory",
                    IconName::MemoryStick,
                    memory,
                    "物理内存 · 已使用 / 总量".into(),
                    cx,
                ))
                .child(item(
                    "monitor-upload",
                    IconName::Upload,
                    rate(data.upload),
                    "上传速率 · 非回环网卡合计".into(),
                    cx,
                ))
                .child(item(
                    "monitor-download",
                    IconName::Download,
                    rate(data.download),
                    "下载速率 · 非回环网卡合计".into(),
                    cx,
                ))
                .child(item(
                    "monitor-uptime",
                    IconName::Clock,
                    uptime(data.uptime),
                    "系统运行时间".into(),
                    cx,
                ))
                .child(item(
                    "monitor-user",
                    IconName::User,
                    data.sessions
                        .map(|count| format!("{} (×{count})", data.user))
                        .unwrap_or_else(|| data.user.clone()),
                    "SSH 用户 · 括号内为已登录会话数".into(),
                    cx,
                ));
            if let Some((mount, used, total)) = &data.disk {
                let label = if *total > 0 {
                    format!("{mount}: {:.0}%", *used as f64 / *total as f64 * 100.0)
                } else {
                    format!("{mount}: —")
                };
                bar = bar.child(item(
                    "monitor-disk",
                    IconName::HardDrive,
                    label,
                    format!(
                        "磁盘已使用 {} / {}",
                        bytes(*used as f64),
                        bytes(*total as f64)
                    ),
                    cx,
                ));
            }
        } else if self.remote {
            bar = bar
                .child(Icon::new(IconName::SquareTerminal).size_3())
                .child(
                    div()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(self.message.clone()),
                );
            if self.failed && self.client.is_some() {
                bar = bar.child(
                    Button::new("monitor-retry")
                        .ghost()
                        .small()
                        .icon(IconName::Replace)
                        .tooltip("重新连接系统监控")
                        .tooltip_placement(Placement::Top)
                        .accessibility_label("重新连接系统监控")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(client) = this.client.clone() {
                                this.task = None;
                                this.failed = false;
                                this.history.clear();
                                this.message = format!("{} · 正在采样…", this.host);
                                this.start_remote(client, cx);
                                cx.notify();
                            }
                        })),
                );
            }
        }
        bar
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_use_elapsed_time_and_ignore_new_or_reset_interfaces() {
        let mut rates = Rates::default();
        let now = Instant::now();
        assert_eq!(
            rates.update(now, BTreeMap::from([("eth0".into(), (100, 200))])),
            (None, None)
        );
        assert_eq!(
            rates.update(
                now + Duration::from_secs(2),
                BTreeMap::from([("eth0".into(), (500, 800)), ("eth1".into(), (9999, 9999))])
            ),
            (Some(200.0), Some(300.0))
        );
        assert_eq!(
            rates.update(
                now + Duration::from_secs(4),
                BTreeMap::from([("eth0".into(), (1, 1))])
            ),
            (Some(0.0), Some(0.0))
        );
    }

    #[test]
    fn linux_frames_use_available_memory_and_cpu_tick_deltas() {
        let mut sampler = RemoteSampler::default();
        let frame = "host server\nuser tester\ncores 4\ncpu 100 0 100 800 0 0 0 0 20 0\nmem MemTotal: 1000 kB\nmem MemAvailable: 400 kB\nuptime 120.5 0\ndisk /dev/root 1000 250 750 25% /\n";
        let first = sampler.parse(frame).unwrap();
        assert_eq!(first.cpu, None);
        assert_eq!(first.memory_used, 600 * 1024);
        let next = sampler
            .parse(&frame.replace("100 0 100 800", "150 0 150 900"))
            .unwrap();
        assert_eq!(next.cpu, Some(50.0));
        assert_eq!(next.disk.unwrap().1, 250 * 1024);
        assert_eq!(next.uptime, 120);
        assert!(sampler.parse("permission denied").is_err());
    }

    #[test]
    fn compact_units_preserve_rate_and_uptime_meaning() {
        assert_eq!(rate(None), "—");
        assert_eq!(rate(Some(2048.0)), "2.0 KiB/s");
        assert_eq!(uptime(90061), "1天 1时");
    }

    #[test]
    fn older_linux_memory_counters_exclude_reclaimable_cache() {
        let mut sampler = RemoteSampler::default();
        let data = sampler.parse("host linux\nuser test\ncpu 1 0 1 8\nmem MemTotal: 1000 kB\nmem MemFree: 100 kB\nmem Buffers: 50 kB\nmem Cached: 200 kB\nmem SReclaimable: 50 kB\nmem Shmem: 25 kB\nuptime 60 0\n").unwrap();
        assert_eq!(data.memory_used, 625 * 1024);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn remote_script_streams_real_linux_metrics_without_a_pty() {
        use std::{
            io::{BufRead as _, BufReader},
            process::{Command, Stdio},
        };
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(REMOTE_SCRIPT)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut frames = Vec::new();
            let mut frame = String::new();
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                if line == FRAME_END.trim() {
                    frames.push(std::mem::take(&mut frame));
                    if frames.len() == 2 {
                        break;
                    }
                } else {
                    frame.push_str(&line);
                    frame.push('\n');
                }
            }
            let _ = sender.send(frames);
        });
        let result = receiver.recv_timeout(Duration::from_secs(10));
        let _ = child.kill();
        let _ = child.wait();
        reader.join().unwrap();
        let frames = result.unwrap();
        assert_eq!(
            frames.len(),
            2,
            "monitor must produce repeated complete frames"
        );
        let mut sampler = RemoteSampler::default();
        let first = sampler.parse(&frames[0]).unwrap();
        let second = sampler.parse(&frames[1]).unwrap();
        assert!(first.memory_total > 0);
        assert!(second.cores > 0);
        assert!(second.cpu.is_some_and(|cpu| (0.0..=100.0).contains(&cpu)));
        assert!(second.uptime >= first.uptime);
        assert!(second.disk.is_some());
    }
}
