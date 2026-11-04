//! Background system sampling for the active terminal's compact status strip.

use std::{
    collections::{BTreeMap, VecDeque},
    time::{Duration, Instant},
};

use futures::{FutureExt as _, StreamExt as _, channel::mpsc, future::Either};
use gpui::StatefulInteractiveElement as _;
use gpui::{
    App, Context, EntityId, InteractiveElement as _, IntoElement, ParentElement, Render,
    Styled as _, Task, Window, div, px,
};
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, tooltip::Tooltip};

use crate::assets::IconName;
use crate::system_icons::SystemIcon;

const REMOTE_TIMEOUT: Duration = Duration::from_secs(12);
const LOADING_MESSAGE: &str = "Loading remote monitoring, please wait...";
const FRAME_END: &str = "__ALACRTERM_STATUS_END__\n";
const REMOTE_SCRIPT: &str = include_str!("../../../scripts/system_status.sh");
type Traffic = BTreeMap<String, (u64, u64)>;

#[derive(Clone, Default, Debug)]
struct Network {
    received: u64,
    transmitted: u64,
    download: Option<f64>,
    upload: Option<f64>,
}

#[derive(Clone, Debug)]
struct Partition {
    device: String,
    mount: String,
    used: u64,
    total: u64,
    available: u64,
    filesystem_type: String,
    used_percent: String,
}

#[derive(Clone, Default, Debug)]
struct Snapshot {
    host: String,
    system_icon: SystemIcon,
    user: String,
    cores: usize,
    threads: usize,
    online_threads: usize,
    sessions: Option<usize>,
    cpu: Option<f32>,
    memory_used: u64,
    memory_total: u64,
    memory_available: u64,
    memory_cached: u64,
    memory_free: u64,
    networks: BTreeMap<String, Network>,
    logins: Vec<String>,
    upload: Option<f64>,
    download: Option<f64>,
    uptime: u64,
    disk: Option<Partition>,
    partitions: Vec<Partition>,
}

#[derive(Default)]
struct Rates {
    previous: Option<(Instant, Traffic)>,
    interfaces: BTreeMap<String, (Option<f64>, Option<f64>)>,
}
impl Rates {
    fn update(&mut self, now: Instant, traffic: Traffic) -> (Option<f64>, Option<f64>) {
        self.interfaces.clear();
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
                self.interfaces.insert(
                    name,
                    (
                        rx.checked_sub(old_rx).map(|bytes| bytes as f64 / elapsed),
                        tx.checked_sub(old_tx).map(|bytes| bytes as f64 / elapsed),
                    ),
                );
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
        let (mut os_id, mut os_like, mut os_kernel) = ("", "", "");
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
                "os_id" => os_id = value.trim(),
                "os_like" => os_like = value.trim(),
                "os_kernel" => os_kernel = value.trim(),
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
                "physical_cores" => snapshot.cores = value.trim().parse()?,
                "threads" | "cores" => {
                    snapshot.online_threads = value.trim().parse()?;
                    snapshot.threads = snapshot.online_threads;
                }
                "configured_threads" => {
                    snapshot.threads = value.trim().parse::<usize>()?.max(snapshot.online_threads)
                }
                "configured_cores" => {
                    let count = value.trim().parse()?;
                    if count > 0 {
                        snapshot.cores = count;
                    }
                }
                "sessions" => snapshot.sessions = Some(value.trim().parse()?),
                "login" => snapshot.logins.push(value.trim().into()),
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
                            snapshot.networks.insert(
                                name.trim().into(),
                                Network {
                                    received: fields[0].parse()?,
                                    transmitted: fields[8].parse()?,
                                    ..Default::default()
                                },
                            );
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
                    let fields: Vec<_> = if value.contains('\t') {
                        value.split('\t').collect()
                    } else {
                        value.split_whitespace().collect()
                    };
                    if fields.len() >= 6 {
                        snapshot.disk = Some(Partition {
                            device: fields[0].into(),
                            mount: fields[5].into(),
                            used: fields[2].parse::<u64>()?.saturating_mul(1024),
                            total: fields[1].parse::<u64>()?.saturating_mul(1024),
                            available: fields[3].parse::<u64>()?.saturating_mul(1024),
                            filesystem_type: String::new(),
                            used_percent: fields[4].into(),
                        });
                    }
                }
                "filesystem" => {
                    let fields: Vec<_> = value.split('\t').collect();
                    if fields.len() == 7 {
                        snapshot.partitions.push(Partition {
                            device: fields[0].into(),
                            filesystem_type: fields[1].into(),
                            total: fields[2].parse::<u64>()?.saturating_mul(1024),
                            used: fields[3].parse::<u64>()?.saturating_mul(1024),
                            available: fields[4].parse::<u64>()?.saturating_mul(1024),
                            used_percent: fields[5].into(),
                            mount: fields[6].into(),
                        });
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
        snapshot.memory_available = available.unwrap_or_else(|| {
            free.saturating_add(buffers)
                .saturating_add(cached)
                .saturating_add(reclaimable)
                .saturating_sub(shared)
        });
        snapshot.memory_used = snapshot
            .memory_total
            .saturating_sub(snapshot.memory_available);
        snapshot.memory_free = free;
        snapshot.memory_cached = cached;
        if network_available {
            (snapshot.download, snapshot.upload) = self.rates.update(Instant::now(), traffic);
            for (name, network) in &mut snapshot.networks {
                if let Some(&(download, upload)) = self.rates.interfaces.get(name) {
                    network.download = download;
                    network.upload = upload;
                }
            }
        } else {
            self.rates = Rates::default();
        }
        snapshot.system_icon = SystemIcon::remote(os_id, os_like, os_kernel);
        Ok(snapshot)
    }
}

pub(crate) struct MonitorTarget {
    pub(crate) id: (EntityId, ssh::SessionId),
    pub(crate) client: ssh::SshExec,
    pub(crate) endpoint: String,
    pub(crate) directory: Option<String>,
}

pub(crate) struct SystemIdentified {
    pub(crate) target: (EntityId, ssh::SessionId),
    pub(crate) icon: SystemIcon,
}
impl gpui::EventEmitter<SystemIdentified> for SystemMonitor {}

pub(crate) struct SystemMonitor {
    target: Option<(EntityId, ssh::SessionId)>,
    remote: bool,
    snapshot: Option<Snapshot>,
    error: Option<String>,
    task: Option<Task<()>>,
    client: Option<ssh::SshExec>,
    history: VecDeque<f32>,
    directory: Option<String>,
    endpoint: String,
}
impl SystemMonitor {
    pub(crate) fn new(_: &mut Context<Self>) -> Self {
        Self {
            target: None,
            remote: false,
            snapshot: None,
            error: None,
            task: None,
            client: None,
            history: VecDeque::new(),
            directory: None,
            endpoint: String::new(),
        }
    }

    pub(crate) fn set_target(
        &mut self,
        target: Option<MonitorTarget>,
        remote: bool,
        cx: &mut Context<Self>,
    ) {
        let id = target.as_ref().map(|target| target.id);
        let directory = target.as_ref().and_then(|target| target.directory.clone());
        if self.target == id && self.remote == remote && self.directory == directory {
            return;
        }
        self.target = id;
        self.remote = remote;
        self.task = None;
        self.snapshot = None;
        self.history.clear();
        self.error = None;
        self.client = None;
        self.endpoint.clear();
        self.directory = directory;
        if let Some(target) = target {
            self.client = Some(target.client.clone());
            self.endpoint = target.endpoint;
            self.start_remote(target.client, cx);
        }
        cx.notify();
    }

    fn start_remote(&mut self, client: ssh::SshExec, cx: &mut Context<Self>) {
        let command = monitor_command(self.directory.as_deref());
        self.task = Some(cx.spawn(async move |this, cx| {
            // Coalesce connection/focus/initial-directory updates before opening
            // a channel; dropping a superseded task prevents a redundant probe.
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let (sender, mut events) = mpsc::unbounded();
            // A dedicated non-PTY channel never writes probes into the user's shell.
            let channel = match client.exec(command, sender) {
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
                                        if this.snapshot.as_ref().map(|old| old.system_icon)
                                            != Some(snapshot.system_icon)
                                            && let Some(target) = this.target
                                        {
                                            cx.emit(SystemIdentified {
                                                target,
                                                icon: snapshot.system_icon,
                                            });
                                        }
                                        if let Some(cpu) = snapshot.cpu {
                                            if this.history.len() == 24 {
                                                this.history.pop_front();
                                            }
                                            this.history.push_back(cpu);
                                        }
                                        this.error = None;
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
        self.error = Some(format!("系统监控不可用 · {message}"));
        cx.notify();
    }
}

fn bytes(value: f64) -> String {
    format_bytes(value, 1)
}

fn precise_bytes(value: f64) -> String {
    format_bytes(value, 2)
}

fn format_bytes(value: f64, precision: usize) -> String {
    let (scaled, unit) = if value >= 1024_f64.powi(3) {
        (value / 1024_f64.powi(3), "GiB")
    } else if value >= 1024_f64.powi(2) {
        (value / 1024_f64.powi(2), "MiB")
    } else if value >= 1024.0 {
        (value / 1024.0, "KiB")
    } else {
        (value, "B")
    };
    format!("{scaled:.precision$} {unit}")
}
fn rate(value: Option<f64>) -> String {
    value
        .map(|value| format!("{}/s", precise_bytes(value)))
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

fn monitor_command(directory: Option<&str>) -> String {
    match directory.filter(|path| path.starts_with('/')) {
        Some(path) => format!(
            "status_dir='{}'\n{REMOTE_SCRIPT}",
            path.replace('\'', "'\\''")
        ),
        None => REMOTE_SCRIPT.into(),
    }
}

impl Snapshot {
    fn is_initialized(&self) -> bool {
        !self.host.is_empty()
            && !self.user.is_empty()
            && self.cpu.is_some()
            && self.memory_total > 0
            && self.upload.is_some()
            && self.download.is_some()
            && self.sessions.is_some()
            && self.disk.as_ref().is_some_and(|disk| disk.total > 0)
    }

    fn memory_tooltip(&self) -> String {
        format!(
            "Memory\ntotal: {}\nused: {}\navailable: {}\ncached: {}\nfree: {}",
            precise_bytes(self.memory_total as f64),
            precise_bytes(self.memory_used as f64),
            precise_bytes(self.memory_available as f64),
            precise_bytes(self.memory_cached as f64),
            precise_bytes(self.memory_free as f64)
        )
    }

    fn network_tooltip(&self, upload: bool) -> String {
        let mut text = if upload {
            "网卡 · 上传速度 · 总上传"
        } else {
            "网卡 · 下载速度 · 总下载"
        }
        .to_string();
        if self.networks.is_empty() {
            text.push_str("\n远端未提供网卡数据");
        }
        for (name, network) in &self.networks {
            let (speed, total) = if upload {
                (network.upload, network.transmitted)
            } else {
                (network.download, network.received)
            };
            text.push_str(&format!(
                "\n{name}    {}    {}",
                rate(speed),
                precise_bytes(total as f64)
            ));
        }
        text
    }

    fn connection_tooltip(&self, endpoint: &str) -> String {
        let mut text = format!(
            "当前 SSH: {endpoint}\n远端登录连接（who -u：用户 / 终端 / 登录时间 / 空闲 / PID / 来源）"
        );
        if self.logins.is_empty() {
            text.push_str("\n未报告登录连接记录");
        }
        for (index, login) in self.logins.iter().enumerate() {
            text.push_str(&format!("\n{}: {login}", index + 1));
        }
        text
    }
}

#[derive(Clone)]
enum StatusTooltip {
    Text(String),
    Partitions(Vec<Partition>),
    Hidden,
}
impl From<String> for StatusTooltip {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

fn partition_row(fields: [String; 7]) -> gpui::AnyElement {
    h_flex()
        .gap_2()
        .children(
            fields
                .into_iter()
                .zip([154., 58., 68., 68., 68., 54., 180.])
                .map(|(field, width)| {
                    div()
                        .w(px(width))
                        .flex_none()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(field)
                }),
        )
        .into_any_element()
}

fn item(
    id: &'static str,
    width: f32,
    icon: impl IntoElement,
    text: String,
    tooltip: impl Into<StatusTooltip>,
    cx: &App,
) -> impl IntoElement + ParentElement {
    let tooltip = tooltip.into();
    let label = div()
        .overflow_hidden()
        .text_ellipsis()
        .whitespace_nowrap()
        .child(text);
    let mut row = h_flex()
        .id(id)
        .w(px(width))
        .h(super::status_bar::STATUS_BAR_ITEM_HEIGHT)
        .flex_none()
        .items_center()
        .gap_0()
        .px(px(8.))
        .border_r_1()
        .border_color(cx.theme().border)
        .text_color(cx.theme().muted_foreground)
        .child(div().flex_none().mr(px(6.)).child(icon));
    if id == "monitor-host" {
        row = row.w_auto().child(label.flex_none());
    } else if id == "monitor-cpu" {
        row = row.child(label.w(px(42.)).flex_none());
    } else {
        row = row.child(label.flex_1().min_w_0());
    }
    if matches!(tooltip, StatusTooltip::Hidden) {
        return row;
    }
    row.tooltip(move |window, cx| {
        let content = tooltip.clone();
        Tooltip::element(move |_, _| {
            let mut body = gpui_kit::component::v_flex().gap_1().text_xs();
            match &content {
                StatusTooltip::Text(text) => {
                    body = body.children(
                        text.lines()
                            .map(|line| div().child(line.to_string()).into_any_element()),
                    );
                }
                StatusTooltip::Partitions(partitions) => {
                    body = body.child(partition_row(
                        [
                            "文件系统",
                            "类型",
                            "大小",
                            "已用",
                            "可用",
                            "已用%",
                            "挂载点",
                        ]
                        .map(str::to_string),
                    ));
                    for partition in partitions {
                        body = body.child(partition_row([
                            partition.device.clone(),
                            partition.filesystem_type.clone(),
                            bytes(partition.total as f64),
                            bytes(partition.used as f64),
                            bytes(partition.available as f64),
                            partition.used_percent.clone(),
                            partition.mount.clone(),
                        ]));
                    }
                }
                StatusTooltip::Hidden => {}
            }
            body
        })
        .build(window, cx)
    })
}

impl Render for SystemMonitor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut bar = h_flex()
            .id("system-monitor-strip")
            .w_full()
            .h(super::status_bar::STATUS_BAR_ITEM_HEIGHT)
            .min_w_0()
            .overflow_x_scroll()
            .items_center()
            .gap_0()
            .font_features(gpui::FontFeatures(std::sync::Arc::new(vec![(
                "tnum".into(),
                1,
            )])))
            .text_size(px(14.))
            .whitespace_nowrap();
        if let Some(data) = self.snapshot.as_ref().filter(|data| data.is_initialized()) {
            let cpu = data
                .cpu
                .map(|cpu| format!("{cpu:.0}%"))
                .unwrap_or_else(|| "—".into());
            let memory = format!(
                "{} / {}",
                precise_bytes(data.memory_used as f64),
                precise_bytes(data.memory_total as f64)
            );
            let cpu_history = div()
                .relative()
                .w(px(48.))
                .h(px(18.))
                .flex_none()
                .overflow_hidden()
                .bg(cx.theme().background)
                .children((1..4).map(|row| {
                    div()
                        .absolute()
                        .left_0()
                        .top(px((row * 18 / 4) as f32))
                        .w_full()
                        .h(px(1.))
                        .bg(cx.theme().border)
                }))
                .children((1..6).map(|column| {
                    div()
                        .absolute()
                        .top_0()
                        .left(px((column * 8) as f32))
                        .h_full()
                        .w(px(1.))
                        .bg(cx.theme().border)
                }))
                .child(
                    h_flex()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .items_end()
                        .gap_0()
                        .children(self.history.iter().map(|sample| {
                            div()
                                .flex_none()
                                .w(px(2.))
                                .h(px((sample.clamp(0.0, 100.0) / 100.0 * 18.).max(1.)))
                                .bg(cx.theme().primary.opacity(0.4))
                        })),
                );
            let cpu_history = div()
                .w(px(50.))
                .h(px(20.))
                .flex_none()
                .border_1()
                .border_color(cx.theme().border)
                .child(cpu_history);
            bar = bar
                .child(item(
                    "monitor-host",
                    140.,
                    data.system_icon.element(px(14.)),
                    data.host.clone(),
                    format!("当前 SSH 远端：{}\n连接：{}\n", data.host, self.endpoint),
                    cx,
                ))
                .child(
                    item(
                        "monitor-cpu",
                        128.,
                        IconName::Cpu,
                        cpu,
                        StatusTooltip::Hidden,
                        cx,
                    )
                    .child(cpu_history),
                )
                .child(item(
                    "monitor-memory",
                    170.,
                    IconName::MemoryStick,
                    memory,
                    data.memory_tooltip(),
                    cx,
                ))
                .child(item(
                    "monitor-upload",
                    120.,
                    IconName::Upload,
                    rate(data.upload),
                    data.network_tooltip(true),
                    cx,
                ))
                .child(item(
                    "monitor-download",
                    120.,
                    IconName::Download,
                    rate(data.download),
                    data.network_tooltip(false),
                    cx,
                ))
                .child(item(
                    "monitor-uptime",
                    96.,
                    IconName::Clock,
                    uptime(data.uptime),
                    "系统运行时间".to_string(),
                    cx,
                ))
                .child(item(
                    "monitor-user",
                    96.,
                    IconName::User,
                    data.sessions
                        .map(|count| format!("{} (×{count})", data.user))
                        .unwrap_or_else(|| data.user.clone()),
                    data.connection_tooltip(&self.endpoint),
                    cx,
                ));
            let (label, tooltip) = if let Some(disk) = &data.disk {
                let percent = if disk.total > 0 {
                    format!("{:.0}%", disk.used as f64 / disk.total as f64 * 100.)
                } else {
                    "—".into()
                };
                (
                    format!("{}: {percent}", disk.mount),
                    format!(
                        "当前分区: {}\n挂载点: {}\n目录: {}\ntotal: {}\nused: {}\navailable: {}\n占用: {percent}",
                        disk.device,
                        disk.mount,
                        self.directory.as_deref().unwrap_or("远端默认目录"),
                        bytes(disk.total as f64),
                        bytes(disk.used as f64),
                        bytes(disk.available as f64)
                    ),
                )
            } else {
                ("分区: —".into(), "远端未提供当前分区信息".into())
            };
            let tooltip = if data.partitions.is_empty() {
                StatusTooltip::Text(tooltip)
            } else {
                StatusTooltip::Partitions(data.partitions.clone())
            };
            bar = bar.child(item(
                "monitor-disk",
                96.,
                IconName::HardDrive,
                label,
                tooltip,
                cx,
            ));
        } else if self.remote {
            if self.error.is_some() {
                bar = bar.child(SystemIcon::default().element(px(14.)));
            }
            bar = bar.child(
                div()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(self.error.clone().unwrap_or_else(|| LOADING_MESSAGE.into())),
            );
            if self.error.is_some() && self.client.is_some() {
                bar = bar.child(
                    h_flex()
                        .id("monitor-retry")
                        .h(super::status_bar::STATUS_BAR_ITEM_HEIGHT)
                        .px(px(2.))
                        .flex_none()
                        .items_center()
                        .child(Icon::new(IconName::Replace).w(px(14.)).h(px(14.)))
                        .tooltip(|window, cx| Tooltip::new("重新连接系统监控").build(window, cx))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(client) = this.client.clone() {
                                this.task = None;
                                this.error = None;
                                this.history.clear();
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
    fn status_strip_waits_until_all_metrics_are_initialized() {
        let mut data = Snapshot {
            host: "server".into(),
            user: "tester".into(),
            memory_total: 1024,
            sessions: Some(1),
            disk: Some(Partition {
                device: "/dev/root".into(),
                mount: "/".into(),
                used: 0,
                total: 1024,
                available: 1024,
                filesystem_type: "ext4".into(),
                used_percent: "0%".into(),
            }),
            ..Default::default()
        };
        assert!(!data.is_initialized());
        data.cpu = Some(0.0);
        assert!(!data.is_initialized());
        data.upload = Some(0.0);
        assert!(!data.is_initialized());
        data.download = Some(0.0);
        assert!(data.is_initialized());
        data.disk = None;
        assert!(!data.is_initialized());
    }

    #[test]
    fn remote_frames_share_the_distribution_icon_and_default_to_bash() {
        let mut sampler = RemoteSampler::default();
        let frame = "host server\ncpu 10 0 10 80\nmem MemTotal: 1000 kB\nuptime 1 0\n";
        assert_eq!(sampler.parse(frame).unwrap().system_icon, SystemIcon::Bash);
        let frame = format!("{frame}os_id linuxmint\nos_like ubuntu debian\nos_kernel Linux\n");
        assert_eq!(
            sampler.parse(&frame).unwrap().system_icon,
            SystemIcon::Ubuntu
        );
    }

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
        assert_eq!(next.disk.unwrap().used, 250 * 1024);
        assert_eq!(next.uptime, 120);
        assert!(sampler.parse("permission denied").is_err());
    }

    #[test]
    fn compact_units_preserve_rate_and_uptime_meaning() {
        assert_eq!(rate(None), "—");
        assert_eq!(rate(Some(2048.0)), "2.00 KiB/s");
        assert_eq!(uptime(90061), "1天 1时");
    }

    #[test]
    fn older_linux_memory_counters_exclude_reclaimable_cache() {
        let mut sampler = RemoteSampler::default();
        let data = sampler.parse("host linux\nuser test\ncpu 1 0 1 8\nmem MemTotal: 1000 kB\nmem MemFree: 100 kB\nmem Buffers: 50 kB\nmem Cached: 200 kB\nmem SReclaimable: 50 kB\nmem Shmem: 25 kB\nuptime 60 0\n").unwrap();
        assert_eq!(data.memory_used, 625 * 1024);
    }

    #[test]
    fn detailed_tooltips_keep_cpu_memory_network_and_login_fields_distinct() {
        let frame = "host server\nuser alice\nthreads 8\nphysical_cores 4\ncpu 1 0 1 8\nmem MemTotal: 1000 kB\nmem MemAvailable: 400 kB\nmem MemFree: 100 kB\nmem Cached: 200 kB\nnet eth0: 1024 1 2 3 0 0 0 0 2048 4 5 6 0 0 0 0\nlink eth0 up\nsessions 2\nlogin alice pts/0 2026-11-02 12:00 . 100 (10.0.0.1)\nlogin bob pts/1 2026-11-02 12:01 . 101 (10.0.0.2)\nuptime 60 0\ndisk /dev/sda1\t1000\t250\t750\t25%\t/mnt/data disk\n";
        let data = RemoteSampler::default().parse(frame).unwrap();
        assert_eq!((data.cores, data.threads), (4, 8));
        assert_eq!(
            (
                data.memory_total,
                data.memory_used,
                data.memory_available,
                data.memory_cached,
                data.memory_free
            ),
            (1000 * 1024, 600 * 1024, 400 * 1024, 200 * 1024, 100 * 1024)
        );
        let tx = data.network_tooltip(true);
        assert!(tx.contains("eth0    —    2.00 KiB"));
        assert!(!tx.contains("errors"));
        let rx = data.network_tooltip(false);
        assert!(rx.contains("eth0    —    1.00 KiB"));
        assert!(!rx.contains("dropped"));
        assert!(
            data.connection_tooltip("alice@server:22")
                .contains("2: bob pts/1")
        );
        let disk = data.disk.unwrap();
        assert_eq!(disk.device, "/dev/sda1");
        assert_eq!(disk.mount, "/mnt/data disk");
        assert_eq!(disk.available, 750 * 1024);
    }

    #[test]
    fn physical_core_count_is_not_guessed_from_thread_count() {
        let data = RemoteSampler::default().parse("host arm\nthreads 8\nphysical_cores 0\ncpu 1 0 1 8\nmem MemTotal: 1000 kB\nuptime 60 0\n").unwrap();
        assert_eq!((data.cores, data.threads, data.online_threads), (0, 8, 8));
    }

    #[test]
    fn configured_cpu_topology_overrides_a_partial_online_view() {
        let data = RemoteSampler::default().parse("host server\nthreads 4\nphysical_cores 2\nconfigured_threads 8\nconfigured_cores 4\ncpu 1 0 1 8\nmem MemTotal: 1000 kB\nuptime 60 0\n").unwrap();
        assert_eq!((data.cores, data.threads, data.online_threads), (4, 8, 4));
    }

    #[test]
    fn filesystem_table_preserves_type_usage_and_mount_points() {
        let data = RemoteSampler::default().parse("host server\nthreads 8\ncpu 1 0 1 8\nmem MemTotal: 1000 kB\nuptime 60 0\nfilesystem /dev/sda2\text4\t100000\t31000\t64000\t33%\t/\nfilesystem tmpfs\ttmpfs\t10000\t48\t9952\t1%\t/run/user disk\n").unwrap();
        assert_eq!(data.partitions.len(), 2);
        assert_eq!(data.partitions[0].filesystem_type, "ext4");
        assert_eq!(data.partitions[0].used_percent, "33%");
        assert_eq!(data.partitions[1].mount, "/run/user disk");
    }

    #[test]
    fn per_interface_rates_are_distinct_and_reset_counters_remain_unknown() {
        let mut rates = Rates::default();
        let now = Instant::now();
        rates.update(
            now,
            BTreeMap::from([("eth0".into(), (100, 200)), ("eth1".into(), (1000, 2000))]),
        );
        rates.update(
            now + Duration::from_secs(2),
            BTreeMap::from([("eth0".into(), (500, 800)), ("eth1".into(), (3000, 6000))]),
        );
        assert_eq!(rates.interfaces["eth0"], (Some(200.0), Some(300.0)));
        assert_eq!(rates.interfaces["eth1"], (Some(1000.0), Some(2000.0)));
        rates.update(
            now + Duration::from_secs(4),
            BTreeMap::from([("eth0".into(), (1, 1))]),
        );
        assert_eq!(rates.interfaces["eth0"], (None, None));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn script_caches_metadata_and_never_runs_cpu_topology_tools() {
        use std::{
            io::Read as _,
            os::unix::fs::PermissionsExt as _,
            process::{Command, Stdio},
        };
        let fixture = tempfile::tempdir().unwrap();
        let directory = fixture.path().join("work ' directory");
        std::fs::create_dir(&directory).unwrap();
        let calls = fixture.path().join("calls");
        let sleeps = fixture.path().join("sleeps");
        let release = fixture.path().join("os-release");
        let injected = fixture.path().join("must-not-exist");
        std::fs::write(
            &release,
            format!(
                "ID='ubuntu'\nID_LIKE=\"debian\"\nPRETTY_NAME=\"$(touch {})\"\n",
                injected.display()
            ),
        )
        .unwrap();
        for (name, body) in [
            (
                "hostname",
                "echo hostname >> \"$MONITOR_TEST_CALLS\"; echo test-host",
            ),
            ("id", "echo id >> \"$MONITOR_TEST_CALLS\"; echo alice"),
            ("lscpu", "echo lscpu >> \"$MONITOR_TEST_CALLS\"; exit 1"),
            (
                "who",
                "echo who >> \"$MONITOR_TEST_CALLS\"; echo 'alice pts/0 2026-11-03 12:00 . 100 (127.0.0.1)'",
            ),
            (
                "df",
                "echo df >> \"$MONITOR_TEST_CALLS\"; printf 'Filesystem Type 1K-blocks Used Available Use%% Mounted\\n/dev/root ext4 1000 400 600 40%% /\\ntmpfs tmpfs 500 100 400 20%% /tmp\\ntmpfs tmpfs 600 200 400 33%% /tmp/not-a-prefix\\n'",
            ),
            (
                "sleep",
                "test \"$1\" = 2 || exit 1; n=0; test ! -f \"$MONITOR_TEST_SLEEPS\" || read -r n < \"$MONITOR_TEST_SLEEPS\"; n=$((n+1)); echo \"$n\" > \"$MONITOR_TEST_SLEEPS\"; test \"$n\" -lt 17",
            ),
        ] {
            let path = fixture.path().join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(monitor_command(directory.to_str()).replace(
                "monitor_release=/etc/os-release",
                &format!("monitor_release='{}'", release.display()),
            ))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    fixture.path().display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("MONITOR_TEST_CALLS", &calls)
            .env("MONITOR_TEST_SLEEPS", sleeps)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut output = String::new();
            let result = stdout.read_to_string(&mut output).map(|_| output);
            let _ = sender.send(result);
        });
        let output = receiver.recv_timeout(Duration::from_secs(10));
        let _ = child.kill();
        let _ = child.wait();
        reader.join().unwrap();
        let output = output.unwrap().unwrap();
        let frames: Vec<_> = output
            .split(FRAME_END)
            .filter(|frame| !frame.trim().is_empty())
            .collect();
        assert_eq!(frames.len(), 17);
        let log = std::fs::read_to_string(calls).unwrap();
        assert_eq!(log.lines().filter(|name| *name == "hostname").count(), 1);
        assert_eq!(log.lines().filter(|name| *name == "id").count(), 1);
        assert_eq!(log.lines().filter(|name| *name == "who").count(), 4);
        assert_eq!(log.lines().filter(|name| *name == "df").count(), 2);
        assert!(!log.lines().any(|name| name == "lscpu"));
        let mut sampler = RemoteSampler::default();
        let first = sampler.parse(frames[0]).unwrap();
        assert!(first.disk.is_none());
        assert_eq!(first.system_icon, SystemIcon::Ubuntu);
        assert!(!injected.exists());
        for frame in frames.iter().skip(1) {
            let sample = sampler.parse(frame).unwrap();
            assert_eq!(sample.sessions, Some(1));
            assert_eq!(sample.logins.len(), 1);
            assert_eq!(sample.partitions.len(), 3);
            assert_eq!(sample.disk.unwrap().mount, "/tmp");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn remote_script_streams_real_linux_metrics_without_a_pty() {
        use std::{
            io::{BufRead as _, BufReader},
            process::{Command, Stdio},
        };
        let directory = tempfile::Builder::new()
            .prefix("status ' $() ")
            .tempdir()
            .unwrap();
        let command = monitor_command(Some(directory.path().to_str().unwrap()));
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(command)
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
        assert!(second.threads > 0);
        assert!(second.cpu.is_some_and(|cpu| (0.0..=100.0).contains(&cpu)));
        assert!(second.uptime >= first.uptime);
        assert!(second.disk.is_some());
    }
}
