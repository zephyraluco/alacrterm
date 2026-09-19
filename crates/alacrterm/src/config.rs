//! 应用配置：终端渲染参数（`config/terminal.json`）与界面主题（`themes/*.json` +
//! `config/app.json` 里的选择）。
//!
//! 两类配置都集中在这里**解析成结构化数据**再交给使用方，使用方不读文件：
//! 渲染参数 → [`RenderSettings`]（传给 `TerminalView::new`）；
//! 主题文件 → `gpui_component::ThemeRegistry`（只登记成主题库，默认不启用）。
//!
//! 运行期状态放在 [`Settings`] 全局里：设置窗口读它、改它，并回写文件（`write_*` 系列）；
//! 主窗口按它创建新会话（见 `AppRoot::spawn_session`）。
//!
//! 查找顺序统一为「可执行文件同级」（发行形态）→「仓库根」（开发形态，经
//! `CARGO_MANIFEST_DIR/../..`）；文件缺失 / 字段缺失 / 取值非法 ⇒ **逐项回退**到默认值
//! 并打一条提示，任何情况下都能启动。写回时选**已存在的那一份**，都没有则选有
//! `config/` 目录的一侧（开发 = 仓库根、发行 = exe 同级）。
//!
//! ⚠️ 提示走 `eprintln!`（带 `[config]` 前缀）：本仓库没有安装 logger（`log` 宏是空操作）。
//!
//! ⚠️ 界面主题默认不启用任何导入的主题（与 gpui-kit 开箱行为一致，用内置的
//! `Default Dark` / `Default Light`）；换主题必须走 [`set_theme`]。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui::{App, FontWeight, Hsla, SharedString, px};
use gpui_kit::component::{Theme, ThemeMode, ThemeRegistry};
use serde::{Deserialize, Serialize};
use serde_json::json;
use terminal::{CursorShape, TerminalColors, rgba_color};
use terminal_view::RenderSettings;

// ---------------------------------------------------------------- 路径解析

/// 终端渲染配置文件相对「可执行文件目录」与「仓库根」的路径。
const TERMINAL_CONFIG_RELATIVE_PATH: &str = "config/terminal.json";

/// 应用偏好（目前只有主题选择），同上。
const APP_CONFIG_RELATIVE_PATH: &str = "config/app.json";

/// 界面主题目录，同上。
const THEMES_DIR: &str = "themes";

/// 候选路径：可执行文件同级 → 仓库根（发行 / 开发两种形态）。
fn candidates(relative: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from))
    {
        paths.push(dir.join(relative));
    }
    paths.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(relative),
    );
    paths
}

/// 按「可执行文件同级」→「仓库根」的顺序读配置文件（发行 / 开发两种形态）。
fn read_config_file(relative: &str) -> Option<(PathBuf, String)> {
    candidates(relative).into_iter().find_map(|path| {
        std::fs::read_to_string(&path)
            .ok()
            .map(|text| (path, text))
    })
}

/// 写回目标：优先**已存在**的那一份；否则选有 `config/` 目录的一侧
/// （开发形态=仓库根、发行形态=exe 同级）。
fn write_target(relative: &str) -> PathBuf {
    let paths = candidates(relative);
    if let Some(existing) = paths.iter().find(|path| path.exists()) {
        return existing.clone();
    }
    paths
        .iter()
        .find(|path| path.parent().is_some_and(Path::is_dir))
        .or_else(|| paths.last())
        .cloned()
        .expect("candidates() 至少给一条路径")
}

/// 打印用路径：规范化后去掉 Windows 的 `\\?\` 前缀（开发形态的候选路径还带 `../..`）。
fn shown_path(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string()
}

// ---------------------------------------------------------------- 运行期状态

/// 运行期可配置项：设置窗口读写、主窗口消费。
///
/// 做成 gpui `Global` 而不是挂在 `AppRoot` 上，是**设置窗口的硬要求**：
/// `SettingField` 的取值闭包签名是 `Fn(&App) -> T`，拿不到任何 Entity。
/// 需要 Entity 的副作用（推给存活会话）由设置窗口借 `WeakEntity<AppRoot>` 完成。
pub(crate) struct Settings {
    /// 终端渲染参数（`config/terminal.json`）——新建会话时传给 `TerminalView::new`。
    pub(crate) render: RenderSettings,
    /// 深色 / 浅色槽位各用哪套主题；`None` = 跟随 gpui-kit 内置主题。
    pub(crate) dark_theme: Option<SharedString>,
    pub(crate) light_theme: Option<SharedString>,
}

impl gpui::Global for Settings {}

/// 启动时读完两个配置文件并装进全局（必须在建窗口前调）。
pub(crate) fn install(cx: &mut App) {
    let render = load_render_settings();
    let app = load_app_config();
    cx.set_global(Settings {
        render,
        dark_theme: app.dark_theme.map(Into::into),
        light_theme: app.light_theme.map(Into::into),
    });
}

/// 读全局配置（设置窗口的字段取值闭包用）。
pub(crate) fn settings(cx: &App) -> &Settings {
    cx.global::<Settings>()
}

/// 改全局配置。
pub(crate) fn settings_mut(cx: &mut App) -> &mut Settings {
    cx.global_mut::<Settings>()
}

// ---------------------------------------------------------------- 终端渲染参数

/// `config/terminal.json` 的结构。字段全部可选：缺什么用什么默认值。
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct TerminalFile {
    font_family: Option<String>,
    font_size: Option<f32>,
    font_weight: Option<f32>,
    line_height_multiplier: Option<f32>,
    minimum_contrast: Option<f32>,
    cursor_shape: Option<String>,
    cursor_blinks: Option<bool>,
    /// ANSI 调色板：键名 = `TerminalColors` 的字段名（见 [`apply_color`]）。
    colors: HashMap<String, String>,
    selection_color: Option<String>,
    search_match_color: Option<String>,
    link_color: Option<String>,
}

/// 读取并应用终端渲染配置；读取失败时返回默认值。
pub(crate) fn load_render_settings() -> RenderSettings {
    let mut settings = RenderSettings::default();

    let Some((path, text)) = read_config_file(TERMINAL_CONFIG_RELATIVE_PATH) else {
        eprintln!("[config] 未找到 {TERMINAL_CONFIG_RELATIVE_PATH}，终端渲染参数使用默认值");
        return settings;
    };

    let file: TerminalFile = match serde_json::from_str(&text) {
        Ok(file) => file,
        Err(error) => {
            eprintln!(
                "[config] 终端渲染参数 {} 解析失败（改用默认值）：{error}",
                path.display()
            );
            return settings;
        }
    };

    apply_render_settings(&mut settings, file);
    eprintln!("[config] 终端渲染参数已加载：{}", shown_path(&path));
    settings
}

/// 把设置窗口管理的终端渲染参数写回 `config/terminal.json`。
///
/// 只覆盖界面负责的那几个键，其余（27 个 ANSI 颜色、3 个 accent 色，以及将来可能
/// 手写的其它键）**原样保留**：读写整份 JSON 值而不是重新序列化整个结构，
/// 免得 `Hsla → hex` 来回换算把颜色改花。
pub(crate) fn save_render_settings(settings: &RenderSettings) {
    let path = write_target(TERMINAL_CONFIG_RELATIVE_PATH);
    let mut doc = read_json_value(&path);
    let Some(map) = doc.as_object_mut() else {
        eprintln!("[config] {} 不是 JSON 对象，放弃保存", path.display());
        return;
    };

    map.insert("font_family".into(), json!(settings.font_family));
    map.insert("font_size".into(), json!(as_number(settings.font_size.as_f32())));
    map.insert("font_weight".into(), json!(as_number(settings.font_weight.0)));
    map.insert(
        "line_height_multiplier".into(),
        json!(as_number(settings.line_height_multiplier)),
    );
    map.insert(
        "minimum_contrast".into(),
        json!(as_number(settings.minimum_contrast)),
    );
    map.insert(
        "cursor_shape".into(),
        json!(cursor_shape_name(settings.cursor_shape)),
    );
    map.insert("cursor_blinks".into(), json!(settings.cursor_blinks));

    write_json_doc(&path, &doc, "终端渲染参数");
}

/// f32 参数 → 写进 JSON / 显示在设置窗口用的数字。
///
/// 不能直接 `as f64`：`1.3f32` 会变成 `1.2999999523162842`（f32 的二进制误差被 f64
/// 放大），写进配置文件难看，输入框里也会显示一长串没意义的小数。按 3 位小数收一下。
pub(crate) fn as_number(value: f32) -> f64 {
    (f64::from(value) * 1000.0).round() / 1000.0
}

/// `CursorShape` → 配置文件里的写法（与 [`parse_cursor_shape`] 对偶；设置窗口的下拉框也用它）。
pub(crate) fn cursor_shape_name(shape: CursorShape) -> &'static str {
    match shape {
        CursorShape::Block => "Block",
        CursorShape::Underline => "Underline",
        CursorShape::Bar => "Bar",
        CursorShape::HollowBlock => "HollowBlock",
        CursorShape::Hidden => "Hidden",
    }
}

/// 读一份 JSON 文档；文件不存在 / 解析失败时给空对象（写回时从零开始）。
fn read_json_value(path: &Path) -> serde_json::Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| json!({}))
}

/// 写回 JSON 文档（缩进 2 空格 + 末尾换行，与仓库里配置文件的手写风格一致）。
fn write_json_doc(path: &Path, doc: &serde_json::Value, what: &str) {
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!("[config] {what}保存失败（建目录 {}）：{error}", parent.display());
        return;
    }

    match serde_json::to_string_pretty(doc) {
        Ok(mut text) => {
            text.push('\n');
            match std::fs::write(path, text) {
                Ok(()) => eprintln!("[config] {what}已保存：{}", shown_path(path)),
                Err(error) => eprintln!("[config] {what}保存失败：{error}"),
            }
        }
        Err(error) => eprintln!("[config] {what}序列化失败：{error}"),
    }
}

fn apply_render_settings(settings: &mut RenderSettings, file: TerminalFile) {
    if let Some(family) = file.font_family {
        settings.font_family = family.into();
    }
    if let Some(size) = file.font_size {
        settings.font_size = px(size);
    }
    if let Some(weight) = file.font_weight {
        settings.font_weight = FontWeight(weight);
    }
    if let Some(multiplier) = file.line_height_multiplier {
        settings.line_height_multiplier = multiplier;
    }
    if let Some(contrast) = file.minimum_contrast {
        settings.minimum_contrast = contrast;
    }
    if let Some(shape) = file.cursor_shape {
        match parse_cursor_shape(&shape) {
            Some(shape) => settings.cursor_shape = shape,
            None => eprintln!(
                "[config] 未知 cursor_shape：{shape}（可选 Block/Underline/Bar/HollowBlock/Hidden）"
            ),
        }
    }
    if let Some(blinks) = file.cursor_blinks {
        settings.cursor_blinks = blinks;
    }

    let colors = &mut settings.colors;
    for (key, value) in &file.colors {
        match parse_color(value) {
            Some(color) => apply_color(colors, key, color),
            None => eprintln!(
                "[config] 颜色 {key} 取值非法：{value}（应为 #RRGGBB 或 #RRGGBBAA）"
            ),
        }
    }

    if let Some(color) = file.selection_color.as_deref().and_then(parse_color) {
        settings.selection_color = color;
    }
    if let Some(color) = file.search_match_color.as_deref().and_then(parse_color) {
        settings.search_match_color = color;
    }
    if let Some(color) = file.link_color.as_deref().and_then(parse_color) {
        settings.link_color = color;
    }
}

/// 把调色板里的一个键写回 [`TerminalColors`]（键名即结构体字段名）。
fn apply_color(colors: &mut TerminalColors, key: &str, color: Hsla) {
    match key {
        "terminal_background" => colors.terminal_background = color,
        "terminal_foreground" => colors.terminal_foreground = color,
        "terminal_bright_foreground" => colors.terminal_bright_foreground = color,
        "terminal_ansi_black" => colors.terminal_ansi_black = color,
        "terminal_ansi_red" => colors.terminal_ansi_red = color,
        "terminal_ansi_green" => colors.terminal_ansi_green = color,
        "terminal_ansi_yellow" => colors.terminal_ansi_yellow = color,
        "terminal_ansi_blue" => colors.terminal_ansi_blue = color,
        "terminal_ansi_magenta" => colors.terminal_ansi_magenta = color,
        "terminal_ansi_cyan" => colors.terminal_ansi_cyan = color,
        "terminal_ansi_white" => colors.terminal_ansi_white = color,
        "terminal_ansi_bright_black" => colors.terminal_ansi_bright_black = color,
        "terminal_ansi_bright_red" => colors.terminal_ansi_bright_red = color,
        "terminal_ansi_bright_green" => colors.terminal_ansi_bright_green = color,
        "terminal_ansi_bright_yellow" => colors.terminal_ansi_bright_yellow = color,
        "terminal_ansi_bright_blue" => colors.terminal_ansi_bright_blue = color,
        "terminal_ansi_bright_magenta" => colors.terminal_ansi_bright_magenta = color,
        "terminal_ansi_bright_cyan" => colors.terminal_ansi_bright_cyan = color,
        "terminal_ansi_bright_white" => colors.terminal_ansi_bright_white = color,
        "terminal_ansi_dim_black" => colors.terminal_ansi_dim_black = color,
        "terminal_ansi_dim_red" => colors.terminal_ansi_dim_red = color,
        "terminal_ansi_dim_green" => colors.terminal_ansi_dim_green = color,
        "terminal_ansi_dim_yellow" => colors.terminal_ansi_dim_yellow = color,
        "terminal_ansi_dim_blue" => colors.terminal_ansi_dim_blue = color,
        "terminal_ansi_dim_magenta" => colors.terminal_ansi_dim_magenta = color,
        "terminal_ansi_dim_cyan" => colors.terminal_ansi_dim_cyan = color,
        "terminal_ansi_dim_white" => colors.terminal_ansi_dim_white = color,
        other => eprintln!("[config] 未知颜色键：{other}"),
    }
}

/// 配置文件里的写法 → `CursorShape`（设置窗口选完光标形状走它）。
pub(crate) fn parse_cursor_shape(value: &str) -> Option<CursorShape> {
    match value.to_ascii_lowercase().as_str() {
        "block" => Some(CursorShape::Block),
        "underline" => Some(CursorShape::Underline),
        "bar" => Some(CursorShape::Bar),
        "hollowblock" => Some(CursorShape::HollowBlock),
        "hidden" => Some(CursorShape::Hidden),
        _ => None,
    }
}

/// `#RRGGBB` / `#RRGGBBAA`（`#` 可省）→ [`Hsla`]。
fn parse_color(value: &str) -> Option<Hsla> {
    let hex = value.trim().trim_start_matches('#');
    let (rgb, alpha) = match hex.len() {
        6 => (hex, 255),
        8 => (&hex[..6], u8::from_str_radix(&hex[6..], 16).ok()?),
        _ => return None,
    };
    let channel = |range: std::ops::Range<usize>| u8::from_str_radix(&rgb[range], 16).ok();
    let mut color = rgba_color(channel(0..2)?, channel(2..4)?, channel(4..6)?);
    color.a = f32::from(alpha) / 255.0;
    Some(color)
}

// ---------------------------------------------------------------- 界面主题

/// 加载主题目录（登记成主题库）并开启热重载；目录不存在时保持内置主题。
///
/// 只**注册**，不启用任何一套：默认配色保持 gpui-kit 开箱的内置
/// `Default Dark` / `Default Light`（见模块文档）。
pub(crate) fn load_themes(cx: &mut App) {
    // 目录不存在就交给内置主题：`watch_dir` 会**创建**目录并挂 notify 监视，
    // 在发行形态下会凭空多出一个空 `themes/`，所以这里先判存在。
    let Some(dir) = themes_dir() else {
        eprintln!("[config] 未找到 {THEMES_DIR}/ 目录，界面使用内置主题");
        return;
    };

    let shown = shown_path(&dir);
    let loaded = preload_themes(&dir, cx);
    let total = ThemeRegistry::global(cx).themes().len();
    eprintln!("[config] 已加载 {loaded} 个主题文件，注册表共 {total} 个主题（{shown}）");

    // 主题热重载会重新投影整套配色（冲掉我们在 `change_theme` 里压的覆盖），
    // 所以这里再压一遍（观察者按注册顺序回调，排在 gpui-kit 的投影之后）。
    cx.observe_global::<ThemeRegistry>(|cx| {
        let mode = Theme::global(cx).mode;
        change_theme(mode, cx);
    })
    .detach();

    match ThemeRegistry::watch_dir(dir, cx, |cx| cx.refresh_windows()) {
        Ok(()) => eprintln!("[config] 主题目录已挂载（热重载生效）"),
        Err(error) => eprintln!("[config] 主题目录挂载失败（{shown}）：{error}"),
    }
}

/// 同步预装目录下的主题文件，返回成功的文件数。
///
/// `watch_dir` 的首次装载是异步的（比首个渲染帧晚），这里先同步读一遍，
/// 免得首帧时注册表还是空的。
fn preload_themes(dir: &Path, cx: &mut App) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };

    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect();
    paths.sort();

    let mut loaded = 0;
    for path in paths {
        let Ok(content) = std::fs::read_to_string(&path) else {
            eprintln!("[config] 主题文件 {} 读取失败", path.display());
            continue;
        };
        match ThemeRegistry::global_mut(cx).load_themes_from_str(&content) {
            Ok(()) => loaded += 1,
            Err(error) => eprintln!("[config] 主题文件 {} 解析失败：{error}", path.display()),
        }
    }
    loaded
}

/// 主题目录：可执行文件同级 → 仓库根，取第一个存在的。
fn themes_dir() -> Option<PathBuf> {
    candidates(THEMES_DIR).into_iter().find(|path| path.is_dir())
}

// ---------------------------------------------------------------- 主题选择

/// 「跟随 gpui-kit」在下拉框里的显示名（也是空串值的标签）。
pub(crate) const FOLLOW_GPUI_KIT: &str = "跟随 gpui-kit（默认）";

/// 主题下拉框的候选项 `(值, 显示名)`：**空串 = 跟随 gpui-kit 内置主题**
/// （注册表里 `is_default` 的那两套不再重复列出）。
pub(crate) fn theme_options(cx: &App, mode: ThemeMode) -> Vec<(SharedString, SharedString)> {
    let mut options = vec![(SharedString::default(), FOLLOW_GPUI_KIT.into())];
    for theme in ThemeRegistry::global(cx).sorted_themes() {
        if theme.is_default || theme.mode != mode {
            continue;
        }
        options.push((theme.name.clone(), theme.name.clone()));
    }
    options
}

/// 应用（或切换）界面主题，并压上两处覆盖：
/// - `sidebar_border` 置透明 —— 分栏边界那条竖线统一交给拖拽条画，免得两条线错位；
///   代价是侧边栏菜单的「嵌套项缩进导线」（同一个 token）也一起消失。
/// - 关掉 `list.active_highlight` —— `ListItem` 默认的 `list_active` 选中底色太淡，
///   关掉后改用主题的 `accent`。
///
/// ⚠️ 每次换主题、以及主题热重载后都要重新压（`Theme::change` 会重新投影整套配色），
/// 所以换主题必须走 [`set_theme`] 或本函数。
///
/// 窗口参数传 `None`：调用方自行 `cx.refresh_windows()`。
pub(crate) fn change_theme(mode: ThemeMode, cx: &mut App) {
    Theme::change(mode, None, cx);
    let theme = Theme::global_mut(cx);
    theme.sidebar_border = Hsla::transparent_black();
    theme.list.active_highlight = false;
}

/// 选定某个模式（深 / 浅）用哪套主题，`None` = 跟随 gpui-kit 内置主题。
///
/// 一次做完三件事：挂到 `Theme` 槽位、记进全局 [`Settings`]、写回 `config/app.json`；
/// 最后走 [`change_theme`] 重新投影。
pub(crate) fn set_theme(name: Option<SharedString>, mode: ThemeMode, cx: &mut App) {
    if !apply_theme_slot(name.as_ref(), mode, cx) {
        return;
    }

    if mode.is_dark() {
        settings_mut(cx).dark_theme = name;
    } else {
        settings_mut(cx).light_theme = name;
    }
    let settings = settings(cx);
    save_theme_choice(
        settings.dark_theme.as_deref(),
        settings.light_theme.as_deref(),
    );

    // 改的是「另一个模式」时也要调：`Theme::change` 顺带把覆盖压回去。
    let mode = Theme::global(cx).mode;
    change_theme(mode, cx);
    cx.refresh_windows();
}

/// 启动时把 `config/app.json` 里的主题选择挂到两个槽位上（不落盘、不投影，
/// 投影交给启动流程里的 `change_theme`）。必须在 [`load_themes`] 之后调。
pub(crate) fn apply_saved_themes(cx: &mut App) {
    let (dark, light) = {
        let settings = settings(cx);
        (settings.dark_theme.clone(), settings.light_theme.clone())
    };
    for (name, mode) in [(dark, ThemeMode::Dark), (light, ThemeMode::Light)] {
        apply_theme_slot(name.as_ref(), mode, cx);
    }
}

/// 把 `name` 对应的主题配置挂到 `mode` 的槽位上；返回是否成功。
///
/// `None`（或名字不在注册表里）时回落到注册表的内置主题——即 gpui-kit 开箱配色。
fn apply_theme_slot(name: Option<&SharedString>, mode: ThemeMode, cx: &mut App) -> bool {
    let registry = ThemeRegistry::global(cx);
    let config = match name {
        Some(name) => match registry.themes().get(name).cloned() {
            Some(config) => config,
            None => {
                eprintln!("[config] 注册表里没有主题「{name}」，该槽位保持原样");
                return false;
            }
        },
        None => match registry.default_themes().get(&mode).cloned() {
            Some(config) => config,
            None => return false,
        },
    };

    if mode.is_dark() {
        Theme::global_mut(cx).dark_theme = config;
    } else {
        Theme::global_mut(cx).light_theme = config;
    }
    true
}

// ---------------------------------------------------------------- 应用偏好（config/app.json）

/// `config/app.json`：应用级偏好，目前只存主题选择。
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct AppFile {
    /// 深色 / 浅色槽位要用的主题名（注册表里的 `themes[].name`）；
    /// 缺省 / `null` = 跟随 gpui-kit 内置主题。
    dark_theme: Option<String>,
    light_theme: Option<String>,
}

/// 读应用偏好。文件不存在是**正常状态**（一切用默认），所以不打提示。
fn load_app_config() -> AppFile {
    let Some((path, text)) = read_config_file(APP_CONFIG_RELATIVE_PATH) else {
        return AppFile::default();
    };

    match serde_json::from_str(&text) {
        Ok(file) => {
            eprintln!("[config] 应用偏好已加载：{}", shown_path(&path));
            file
        }
        Err(error) => {
            eprintln!(
                "[config] 应用偏好 {} 解析失败（改用默认）：{error}",
                path.display()
            );
            AppFile::default()
        }
    }
}

/// 把主题选择写回 `config/app.json`（`None` 写 `null` = 跟随 gpui-kit）。
pub(crate) fn save_theme_choice(dark: Option<&str>, light: Option<&str>) {
    let path = write_target(APP_CONFIG_RELATIVE_PATH);
    let doc = json!({ "dark_theme": dark, "light_theme": light });
    write_json_doc(&path, &doc, "应用偏好");
}
