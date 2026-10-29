//! One update model shared by settings and every application window.
use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use semver::Version;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
#[cfg(not(target_os = "macos"))]
use updater::manifest::Offer;
use updater::manifest::Release;

#[derive(Clone, Debug)]
pub(crate) enum Phase {
    Disabled,
    Idle,
    Checking,
    Current,
    Available(UpdateOffer),
    Downloading {
        update: SignedRelease,
        percent: u8,
    },
    Ready {
        update: SignedRelease,
        package: PathBuf,
    },
    Preparing {
        update: SignedRelease,
        package: PathBuf,
    },
    Failed(String),
    #[cfg(any(target_os = "macos", test))]
    Native(updater::macos::State),
}

impl Phase {
    pub(crate) fn available_version(&self) -> Option<String> {
        match self {
            Phase::Available(UpdateOffer::Signed(update))
            | Phase::Downloading { update, .. }
            | Phase::Ready { update, .. }
            | Phase::Preparing { update, .. } => Some(update.release.version.to_string()),
            Phase::Available(UpdateOffer::Manual(version)) => Some(version.to_string()),
            #[cfg(any(target_os = "macos", test))]
            Phase::Native(state) if state.has_update() => updater::macos::available_version(),
            _ => None,
        }
    }

    fn title_bar_label(&self) -> &'static str {
        match self {
            Self::Downloading { .. } => "下载中",
            Self::Preparing { .. } => "准备中",
            #[cfg(any(target_os = "macos", test))]
            Self::Native(updater::macos::State::Downloading) => "下载中",
            _ => "可升级",
        }
    }

    pub(crate) fn has_update(&self) -> bool {
        match self {
            Self::Available(_)
            | Self::Downloading { .. }
            | Self::Ready { .. }
            | Self::Preparing { .. } => true,
            #[cfg(any(target_os = "macos", test))]
            Self::Native(state) => state.has_update(),
            _ => false,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SignedRelease {
    release: Release,
    envelope: Vec<u8>,
}
#[derive(Clone, Debug)]
pub(crate) enum UpdateOffer {
    Signed(SignedRelease),
    Manual(Version),
}

pub(crate) enum PreparedInstall {
    #[cfg(windows)]
    Windows(updater::windows::Handoff),
    #[cfg(target_os = "linux")]
    Linux(PathBuf),
}
impl PreparedInstall {
    pub(crate) fn install(self, cx: &mut App) -> anyhow::Result<()> {
        #[cfg(target_os = "macos")]
        {
            let _ = cx;
            match self {}
        }
        #[cfg(not(target_os = "macos"))]
        {
            match self {
                #[cfg(windows)]
                Self::Windows(handoff) => {
                    handoff.commit()?;
                    cx.quit();
                }
                #[cfg(target_os = "linux")]
                Self::Linux(package) => {
                    let entity = model(cx);
                    entity.update(cx, |this, cx| {
                        this.notice = if let Some(installer) =
                            updater::linux::installer(&package, |path| path.is_file())
                        {
                            std::process::Command::new(installer.program)
                                .args(installer.arguments)
                                .spawn()?;
                            "已打开系统安装器。安装完成后请退出并重新启动 Alacrterm。".into()
                        } else {
                            format!(
                                "未检测到支持的系统安装器。请在终端执行：\n{}",
                                updater::linux::manual_command(&package)
                            )
                        };
                        cx.notify();
                        anyhow::Ok(())
                    })?;
                }
            }
            Ok(())
        }
    }
}

enum Event {
    #[cfg(not(target_os = "macos"))]
    Checked(Result<(Offer, Vec<u8>), String>),
    Progress(u8),
    Downloaded(Result<PathBuf, String>),
}

pub(crate) struct Updates {
    pub(crate) phase: Phase,
    pub(crate) automatic: bool,
    pub(crate) notice: String,
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
                        if this.automatic && !cfg!(target_os = "macos") {
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
        #[cfg(not(target_os = "macos"))]
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
        if let Ok(data) = updater::data_dir() {
            let mut folders = vec![data.clone()];
            if let Ok(jobs) = std::fs::read_dir(data.join("jobs")) {
                let mut jobs: Vec<_> = jobs.flatten().map(|job| job.path()).collect();
                jobs.sort_by_key(|path| {
                    std::fs::metadata(path)
                        .and_then(|metadata| metadata.modified())
                        .ok()
                });
                folders.extend(jobs);
            }
            for folder in folders {
                let error_path = folder.join("helper-error.log");
                if let Ok(error) = std::fs::read_to_string(&error_path) {
                    notice = format!("上次更新助手失败：{error}");
                    if let Err(error) =
                        std::fs::rename(&error_path, folder.join("last-helper-error.log"))
                    {
                        log::warn!("保留更新错误日志：{error}");
                    }
                }
                let result_path = folder.join("result.json");
                if let Ok(bytes) = std::fs::read(&result_path)
                    && let Ok(result) =
                        serde_json::from_slice::<updater::windows::InstallResult>(&bytes)
                {
                    notice =
                        if result.installed && result.to.to_string() == env!("CARGO_PKG_VERSION") {
                            format!("已更新到 {}", result.to)
                        } else {
                            format!("上次更新未完成：{}", result.message)
                        };
                    if let Err(error) =
                        std::fs::rename(&result_path, folder.join("last-result.json"))
                    {
                        log::warn!("记录更新结果：{error}");
                    }
                }
            }
        }
        #[cfg(target_os = "macos")]
        let enabled = if cfg!(debug_assertions) {
            false
        } else {
            match updater::macos::start(automatic) {
                Ok(()) => {
                    notice.clear();
                    true
                }
                Err(error) => {
                    notice = error.to_string();
                    false
                }
            }
        };
        Updates {
            phase: if enabled {
                Phase::Idle
            } else {
                Phase::Disabled
            },
            automatic,
            notice,
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
        #[cfg(any(target_os = "macos", test))]
        if let Phase::Native(state) = self.phase {
            return state.busy();
        }
        matches!(
            self.phase,
            Phase::Checking | Phase::Downloading { .. } | Phase::Preparing { .. }
        )
    }

    pub(crate) fn status(&self) -> String {
        #[cfg(target_os = "macos")]
        if !matches!(self.phase, Phase::Disabled) {
            return updater::macos::status();
        }
        match &self.phase {
            Phase::Disabled => self.notice.clone(),
            Phase::Idle => "尚未检查更新".into(),
            Phase::Checking => "正在检查更新…".into(),
            Phase::Current => "已是最新版本".into(),
            Phase::Available(UpdateOffer::Signed(update)) => {
                format!("发现新版本 {}", update.release.version)
            }
            Phase::Available(_) => "请从更新页面继续安装新版本".into(),
            Phase::Downloading { percent, .. } => format!("正在下载更新：{percent}%"),
            Phase::Ready { .. } => "更新已校验，等待安装".into(),
            Phase::Preparing { .. } => "正在校验安装包并准备更新助手…".into(),
            Phase::Failed(error) => error.clone(),
            #[cfg(any(target_os = "macos", test))]
            Phase::Native(_) => updater::macos::status(),
        }
    }

    pub(crate) fn set_automatic(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let store = settings::SettingsStore::new(settings::write_target("config/update.json"));
        match store.update(serde_json::Map::from_iter([(
            "automatic".into(),
            serde_json::json!(enabled),
        )])) {
            Ok(()) => {
                self.automatic = enabled;
                #[cfg(target_os = "macos")]
                updater::macos::set_automatic(enabled);
            }
            Err(error) => self.notice = format!("保存更新设置失败：{error:#}"),
        }
        cx.notify();
    }

    pub(crate) fn check(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        {
            if !matches!(self.phase, Phase::Disabled) {
                updater::macos::check();
                cx.notify();
            }
            return;
        }
        #[cfg(not(target_os = "macos"))]
        self.check_feed(cx);
    }

    #[cfg(not(target_os = "macos"))]
    fn check_feed(&mut self, cx: &mut Context<Self>) {
        if self.busy() || matches!(self.phase, Phase::Disabled | Phase::Ready { .. }) {
            return;
        }
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
        let Phase::Available(UpdateOffer::Signed(update)) = self.phase.clone() else {
            return;
        };
        let release = update.release.clone();
        let (sender, receiver) = mpsc::channel();
        self.events = Some(receiver);
        self.poll(cx);
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        self.phase = Phase::Downloading { update, percent: 0 };
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
                #[cfg(not(target_os = "macos"))]
                Some(Ok(Event::Checked(result))) => {
                    self.events = None;
                    match result {
                        Ok((Offer::Current, _)) => self.phase = Phase::Current,
                        Ok((Offer::Manual(version), _)) => {
                            self.phase = Phase::Available(UpdateOffer::Manual(version.clone()));
                            self.notice = format!("升级到 {version} 需要手动下载安装");
                        }
                        Ok((Offer::Update(release), envelope)) => {
                            self.phase = Phase::Available(UpdateOffer::Signed(SignedRelease {
                                release,
                                envelope,
                            }));
                        }
                        Err(error) => self.phase = Phase::Failed(error),
                    }
                    cx.notify();
                }
                Some(Ok(Event::Progress(percent))) => {
                    if let Phase::Downloading {
                        percent: current, ..
                    } = &mut self.phase
                    {
                        *current = percent;
                    }
                    cx.notify();
                }
                Some(Ok(Event::Downloaded(result))) => {
                    self.events = None;
                    let Phase::Downloading { update, .. } = self.phase.clone() else {
                        continue;
                    };
                    match result {
                        _ if self.cancel.load(Ordering::Relaxed) => {
                            self.phase = Phase::Available(UpdateOffer::Signed(update));
                            self.notice.clear();
                        }
                        Ok(path) => {
                            self.phase = Phase::Ready {
                                update,
                                package: path,
                            };
                        }
                        Err(error) => {
                            self.notice = error;
                            self.phase = Phase::Available(UpdateOffer::Signed(update));
                        }
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

    pub(crate) fn available_version(&self) -> Option<String> {
        self.phase.available_version()
    }

    pub(crate) fn release(&self) -> Option<&Release> {
        match &self.phase {
            Phase::Available(UpdateOffer::Signed(update))
            | Phase::Downloading { update, .. }
            | Phase::Ready { update, .. }
            | Phase::Preparing { update, .. } => Some(&update.release),
            _ => None,
        }
    }

    pub(crate) fn package(&self) -> Option<&std::path::Path> {
        match &self.phase {
            Phase::Ready { package, .. } | Phase::Preparing { package, .. } => Some(package),
            _ => None,
        }
    }

    pub(crate) fn prepare_install(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<Task<anyhow::Result<PreparedInstall>>> {
        let Phase::Ready { update, package } = self.phase.clone() else {
            return None;
        };
        self.phase = Phase::Preparing {
            update: update.clone(),
            package: package.clone(),
        };
        cx.notify();
        Some(cx.background_executor().spawn(async move {
            #[cfg(windows)]
            {
                Ok(PreparedInstall::Windows(updater::windows::prepare_handoff(
                    update.envelope,
                    package,
                    Version::parse(env!("CARGO_PKG_VERSION"))?,
                )?))
            }
            #[cfg(target_os = "linux")]
            {
                let _ = update.envelope;
                updater::download::verify(&package, &update.release.asset)?;
                Ok(PreparedInstall::Linux(package))
            }
            #[cfg(not(any(windows, target_os = "linux")))]
            {
                let _ = (update, package);
                anyhow::bail!("此平台使用 Sparkle 安装更新")
            }
        }))
    }

    pub(crate) fn finish_preparing(&mut self, error: Option<String>, cx: &mut Context<Self>) {
        if let Phase::Preparing { update, package } = self.phase.clone() {
            self.phase = Phase::Ready { update, package };
        }
        if let Some(error) = error {
            self.notice = error;
        }
        cx.notify();
    }
}

/// Prepare on a worker, then recheck application exit conditions on the UI thread.
/// `continue_installation` lets a dismissed dialog cancel its installation intent.
pub(crate) fn install_package(
    root: gpui::WeakEntity<crate::AppRoot>,
    window: &mut gpui::Window,
    cx: &mut App,
    continue_installation: impl FnOnce(bool, &mut gpui::Window, &mut App) -> bool + 'static,
) {
    let installing = model(cx);
    let Some(task) = installing.update(cx, |update, cx| update.prepare_install(cx)) else {
        return;
    };
    window
        .spawn(cx, async move |cx| {
            let recovery = installing.clone();
            let prepared = task.await;
            if let Err(error) = cx.update(|window, cx| {
                let proceed = continue_installation(prepared.is_ok(), window, cx);
                match prepared {
                    Ok(prepared) if proceed => {
                        crate::exit_coordinator::request_with_completion(
                            root,
                            window,
                            cx,
                            "安装更新",
                            Box::new(move |window, cx| {
                                prepared.install(cx)?;
                                #[cfg(target_os = "linux")]
                                {
                                    use gpui_kit::component::{
                                        WindowExt as _, notification::Notification,
                                    };
                                    window.push_notification(
                                        Notification::info(model(cx).read(cx).notice.clone()),
                                        cx,
                                    );
                                }
                                #[cfg(not(target_os = "linux"))]
                                let _ = window;
                                Ok(())
                            }),
                            Box::new(move |_, cx| {
                                installing
                                    .update(cx, |update, cx| update.finish_preparing(None, cx))
                            }),
                        );
                    }
                    Ok(_) => installing.update(cx, |update, cx| update.finish_preparing(None, cx)),
                    Err(error) => {
                        installing.update(cx, |update, cx| {
                            update.finish_preparing(Some(format!("安装准备失败：{error:#}")), cx)
                        });
                    }
                }
            }) {
                gpui::AsyncApp::update(cx, |cx| {
                    recovery.update(cx, |update, cx| update.finish_preparing(None, cx))
                });
                log::debug!("安装更新窗口已关闭：{error}");
            }
        })
        .detach();
}

impl Drop for Updates {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl gpui::Render for Updates {
    fn render(&mut self, _: &mut gpui::Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        use gpui::{InteractiveElement as _, ParentElement as _, prelude::FluentBuilder as _};
        use gpui_kit::component::{
            Sizable as _,
            button::{Button, ButtonVariants as _},
        };
        gpui::div().when(self.phase.has_update(), |view| {
            view.occlude()
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    Button::new("update-status")
                        .ghost()
                        .small()
                        .icon(crate::assets::IconName::Download)
                        .label(self.phase.title_bar_label())
                        .tooltip(self.status())
                        .accessibility_label("打开软件更新")
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(crate::actions::OpenUpdateDialog), cx)
                        }),
                )
        })
    }
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn attach_sparkle_guard(
    root: gpui::WeakEntity<crate::AppRoot>,
    window: &mut gpui::Window,
    cx: &mut App,
) {
    window
        .spawn(cx, async move |cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                let result = root.update_in(cx, |_root, window, cx| {
                    let update = model(cx);
                    if matches!(update.read(cx).phase, Phase::Disabled) {
                        return;
                    }
                    if updater::macos::changed() {
                        update.update(cx, |update, cx| {
                            update.phase = Phase::Native(updater::macos::State::from_code(
                                updater::macos::state(),
                            ));
                            cx.notify();
                        });
                    }
                    if !updater::macos::take_restart_request() {
                        return;
                    }
                    let root = cx.weak_entity();
                    window.defer(cx, move |window, cx| {
                        let approved_root = root.clone();
                        crate::exit_coordinator::request(
                            root,
                            window,
                            cx,
                            "重启并更新",
                            Box::new(move |_, cx| {
                                platform::shutdown_wayx();
                                crate::exit_coordinator::allow_sparkle_restart(&approved_root, cx);
                                Ok(())
                            }),
                        )
                    });
                });
                if result.is_err() {
                    break;
                }
            }
        })
        .detach();
}

#[cfg(test)]
mod tests {
    use super::{Phase, UpdateOffer};
    use semver::Version;

    #[test]
    fn title_bar_entry_tracks_update_availability_and_native_completion() {
        for phase in [
            Phase::Disabled,
            Phase::Idle,
            Phase::Checking,
            Phase::Current,
            Phase::Failed("offline".into()),
        ] {
            assert!(!phase.has_update());
        }
        let available = Phase::Available(UpdateOffer::Manual(Version::new(0, 2, 0)));
        assert!(available.has_update());
        assert_eq!(available.title_bar_label(), "可升级");
        for state in [
            updater::macos::State::Available,
            updater::macos::State::WaitingRestart,
            updater::macos::State::Deferred,
        ] {
            let phase = Phase::Native(state);
            assert!(phase.has_update());
            assert_eq!(phase.title_bar_label(), "可升级");
        }
        assert_eq!(
            Phase::Native(updater::macos::State::Downloading).title_bar_label(),
            "下载中"
        );
        assert!(!Phase::Native(updater::macos::State::Idle).has_update());
    }

    #[test]
    fn upgrade_version_is_available_for_manual_and_all_package_states() {
        let update = super::SignedRelease {
            release: updater::manifest::Release {
                version: Version::new(2, 3, 4),
                notes: String::new(),
                asset: updater::manifest::Asset {
                    url: "https://example.com/update.deb".into(),
                    size: 5,
                    sha256: "0".repeat(64),
                    kind: updater::manifest::PackageKind::Deb,
                },
            },
            envelope: vec![],
        };
        for phase in [
            Phase::Available(UpdateOffer::Signed(update.clone())),
            Phase::Downloading {
                update: update.clone(),
                percent: 42,
            },
            Phase::Ready {
                update: update.clone(),
                package: "/tmp/update.deb".into(),
            },
            Phase::Preparing {
                update,
                package: "/tmp/update.deb".into(),
            },
            Phase::Available(UpdateOffer::Manual(Version::new(2, 3, 4))),
        ] {
            assert_eq!(phase.available_version().as_deref(), Some("2.3.4"));
        }
        assert!(Phase::Current.available_version().is_none());
    }

    #[test]
    fn sparkle_guard_uses_the_window_async_context_on_every_test_platform() {
        let _: fn(gpui::WeakEntity<crate::AppRoot>, &mut gpui::Window, &mut gpui::App) =
            super::attach_sparkle_guard;
    }
}
