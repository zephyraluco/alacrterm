//! One update model shared by settings and every application window.
use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use semver::Version;
#[cfg(windows)]
use std::sync::Mutex;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
use updater::manifest::{Offer, Release};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Disabled,
    Idle,
    Checking,
    Current,
    Available,
    Downloading(u8),
    Ready,
    Failed(String),
}

enum Event {
    Checked(Result<(Offer, Vec<u8>), String>),
    Progress(u8),
    Downloaded(Result<PathBuf, String>),
}

pub(crate) struct Updates {
    pub(crate) phase: Phase,
    pub(crate) automatic: bool,
    pub(crate) release: Option<Release>,
    pub(crate) package: Option<PathBuf>,
    pub(crate) manual: bool,
    pub(crate) notice: String,
    envelope: Vec<u8>,
    events: Option<mpsc::Receiver<Event>>,
    cancel: Arc<AtomicBool>,
    _poll: Option<Task<()>>,
    _schedule: Task<()>,
}

struct GlobalUpdates(Entity<Updates>);
impl Global for GlobalUpdates {}

pub(crate) fn model(cx: &App) -> Entity<Updates> {
    cx.global::<GlobalUpdates>().0.clone()
}

pub(crate) fn install(cx: &mut App) {
    let automatic = settings::read_config_file("config/update.json")
        .and_then(|(_, text)| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| value["automatic"].as_bool())
        .unwrap_or(true);
    let entity = cx.new(|cx: &mut Context<Updates>| {
        let schedule = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs(30))
                .await;
            loop {
                if this
                    .update(cx, |this, cx| {
                        if this.automatic {
                            this.check(cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_secs(6 * 60 * 60))
                    .await;
            }
        });
        let enabled = !cfg!(debug_assertions)
            && updater::platform_key().is_some()
            && !updater::trusted_keys().is_empty();
        let mut notice = if cfg!(debug_assertions) {
            "开发构建不执行在线更新".into()
        } else if updater::trusted_keys().is_empty() {
            "此构建未配置更新公钥，请从发布页下载安装".into()
        } else {
            String::new()
        };
        if let Ok(folder) = updater::data_dir() {
            let result_path = folder.join("result.json");
            if let Ok(bytes) = std::fs::read(&result_path)
                && let Ok(result) =
                    serde_json::from_slice::<updater::windows::InstallResult>(&bytes)
            {
                notice = if result.installed && result.to.to_string() == env!("CARGO_PKG_VERSION") {
                    format!("已更新到 {}", result.to)
                } else {
                    format!("上次更新未完成：{}", result.message)
                };
                if let Err(error) = std::fs::rename(&result_path, folder.join("last-result.json")) {
                    log::warn!("记录更新结果：{error}");
                }
            }
        }
        Updates {
            phase: if enabled {
                Phase::Idle
            } else {
                Phase::Disabled
            },
            automatic,
            release: None,
            package: None,
            manual: false,
            notice,
            envelope: Vec::new(),
            events: None,
            cancel: Arc::new(AtomicBool::new(false)),
            _poll: None,
            _schedule: schedule,
        }
    });
    cx.set_global(GlobalUpdates(entity));
}

impl Updates {
    pub(crate) fn busy(&self) -> bool {
        matches!(self.phase, Phase::Checking | Phase::Downloading(_))
    }

    pub(crate) fn status(&self) -> String {
        match &self.phase {
            Phase::Disabled => self.notice.clone(),
            Phase::Idle => "尚未检查更新".into(),
            Phase::Checking => "正在检查更新…".into(),
            Phase::Current => "已是最新版本".into(),
            Phase::Available => self.release.as_ref().map_or_else(
                || "请从发布页安装新版本".into(),
                |release| format!("发现新版本 {}", release.version),
            ),
            Phase::Downloading(percent) => format!("正在下载更新：{percent}%"),
            Phase::Ready => "更新已校验，等待安装".into(),
            Phase::Failed(error) => error.clone(),
        }
    }

    pub(crate) fn set_automatic(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let store = settings::SettingsStore::new(settings::write_target("config/update.json"));
        match store.update(serde_json::Map::from_iter([(
            "automatic".into(),
            serde_json::json!(enabled),
        )])) {
            Ok(()) => self.automatic = enabled,
            Err(error) => self.notice = format!("保存更新设置失败：{error:#}"),
        }
        cx.notify();
    }

    pub(crate) fn check(&mut self, cx: &mut Context<Self>) {
        if self.busy() || matches!(self.phase, Phase::Disabled | Phase::Ready) {
            return;
        }
        self.release = None;
        self.package = None;
        self.manual = false;
        self.phase = Phase::Checking;
        self.notice.clear();
        let (sender, receiver) = mpsc::channel();
        self.events = Some(receiver);
        self.poll(cx);
        let spawned = std::thread::Builder::new()
            .name("update-check".into())
            .spawn(move || {
                let result = (|| {
                    let feed = updater::download::Feed::new()?;
                    let (manifest, envelope) =
                        feed.check_signed(updater::FEED_URL, &updater::trusted_keys())?;
                    let offer = manifest.offer(
                        &Version::parse(env!("CARGO_PKG_VERSION"))?,
                        updater::platform_key()
                            .ok_or_else(|| anyhow::anyhow!("此平台请使用系统更新器"))?,
                    )?;
                    anyhow::Ok((offer, envelope))
                })()
                .map_err(|error| format!("检查更新失败：{error:#}"));
                if let Err(error) = sender.send(Event::Checked(result)) {
                    log::debug!("更新视图已关闭：{error}");
                }
            });
        if let Err(error) = spawned {
            self.phase = Phase::Failed(error.to_string());
            self.events = None;
        }
        cx.notify();
    }

    pub(crate) fn download(&mut self, cx: &mut Context<Self>) {
        if self.busy() || self.manual {
            return;
        }
        let Some(release) = self.release.clone() else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        self.events = Some(receiver);
        self.poll(cx);
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        self.phase = Phase::Downloading(0);
        let spawned = std::thread::Builder::new()
            .name("update-download".into())
            .spawn(move || {
                let result = (|| {
                    updater::download::Feed::new()?.download(
                        &release,
                        &updater::cache_dir()?,
                        &cancel,
                        |done, total| {
                            if let Err(error) = sender
                                .send(Event::Progress((done.saturating_mul(100) / total) as u8))
                            {
                                log::debug!("更新视图已关闭：{error}");
                            }
                        },
                    )
                })()
                .map_err(|error| format!("下载更新失败：{error:#}"));
                if let Err(error) = sender.send(Event::Downloaded(result)) {
                    log::debug!("更新视图已关闭：{error}");
                }
            });
        if let Err(error) = spawned {
            self.phase = Phase::Failed(error.to_string());
            self.events = None;
        }
        cx.notify();
    }

    pub(crate) fn cancel_download(&mut self, cx: &mut Context<Self>) {
        self.cancel.store(true, Ordering::Relaxed);
        self.notice = "正在取消下载…".into();
        cx.notify();
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        self._poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                if !matches!(
                    this.update(cx, |this, cx| {
                        this.collect(cx);
                        this.events.is_some()
                    }),
                    Ok(true)
                ) {
                    break;
                }
            }
        }));
    }

    fn collect(&mut self, cx: &mut Context<Self>) {
        loop {
            let event = self.events.as_ref().map(|events| events.try_recv());
            match event {
                Some(Ok(Event::Checked(result))) => {
                    self.events = None;
                    match result {
                        Ok((Offer::Current, _)) => self.phase = Phase::Current,
                        Ok((Offer::Manual(version), _)) => {
                            self.phase = Phase::Available;
                            self.manual = true;
                            self.notice = format!("升级到 {version} 需要手动下载安装");
                        }
                        Ok((Offer::Update(release), envelope)) => {
                            self.release = Some(release);
                            self.envelope = envelope;
                            self.phase = Phase::Available;
                        }
                        Err(error) => self.phase = Phase::Failed(error),
                    }
                    cx.notify();
                }
                Some(Ok(Event::Progress(percent))) => {
                    self.phase = Phase::Downloading(percent);
                    cx.notify();
                }
                Some(Ok(Event::Downloaded(result))) => {
                    self.events = None;
                    match result {
                        Ok(path) => {
                            self.package = Some(path);
                            self.phase = Phase::Ready;
                        }
                        Err(_) if self.cancel.load(Ordering::Relaxed) => {
                            self.phase = Phase::Available;
                            self.notice.clear();
                        }
                        Err(error) => self.phase = Phase::Failed(error),
                    }
                    cx.notify();
                }
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    self.events = None;
                    self.phase = Phase::Failed("更新任务意外结束，请重试".into());
                    cx.notify();
                }
                _ => break,
            }
        }
    }

    pub(crate) fn install_package(&mut self, cx: &mut Context<Self>) -> anyhow::Result<bool> {
        let package = self
            .package
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("安装包尚未准备好"))?;
        let release = self
            .release
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("更新信息不存在"))?;
        updater::download::verify(package, &release.asset)?;
        #[cfg(windows)]
        {
            let plan = updater::windows::prepare(
                self.envelope.clone(),
                package.clone(),
                Version::parse(env!("CARGO_PKG_VERSION"))?,
            )?;
            *HANDOFF
                .lock()
                .map_err(|_| anyhow::anyhow!("更新交接状态损坏"))? = Some(plan);
            return Ok(true);
        }
        #[cfg(target_os = "linux")]
        {
            if let Some(installer) = updater::linux::installer(package, |path| path.is_file()) {
                std::process::Command::new(installer.program)
                    .args(installer.arguments)
                    .spawn()?;
                self.notice = "已打开系统安装器。安装完成后请退出并重新启动 Alacrterm。".into();
            } else {
                self.notice = format!(
                    "未检测到支持的系统安装器。请在终端执行：\n{}",
                    updater::linux::manual_command(package)
                );
            }
            cx.notify();
            Ok(false)
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            let _ = cx;
            anyhow::bail!("此平台使用 Sparkle 安装更新")
        }
    }
}

impl Drop for Updates {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(windows)]
static HANDOFF: Mutex<Option<PathBuf>> = Mutex::new(None);

pub(crate) fn finish_handoff() {
    #[cfg(windows)]
    if let Ok(mut handoff) = HANDOFF.lock() {
        if let Some(plan) = handoff.take() {
            if let Err(error) = updater::windows::start(&plan) {
                log::error!("启动更新助手失败：{error:#}");
            }
        }
    }
}

impl gpui::Render for Updates {
    fn render(&mut self, _: &mut gpui::Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        use gpui::{ParentElement as _, Styled as _, prelude::FluentBuilder as _};
        use gpui_kit::component::{Sizable as _, button::Button};
        let visible = matches!(
            self.phase,
            Phase::Available | Phase::Ready | Phase::Downloading(_)
        );
        gpui::div().when(visible, |view| {
            view.child(
                Button::new("update-status")
                    .small()
                    .h(crate::status_bar::STATUS_BAR_ITEM_HEIGHT)
                    .label(self.status())
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(crate::actions::OpenSettings), cx)
                    }),
            )
        })
    }
}
