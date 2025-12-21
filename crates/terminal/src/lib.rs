mod pty_info;
mod utils;

use crate::pty_info::PtyProcessInfo;
use crate::utils::find_in_env;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty;
use tokio::sync::mpsc::UnboundedSender;

struct TermSize {
    columns: usize,
    screen_lines: usize,
}

impl TermSize {
    fn new(columns: usize, screen_lines: usize) -> Self {
        Self {
            columns,
            screen_lines,
        }
    }
}

impl Dimensions for TermSize {
    fn columns(&self) -> usize {
        self.columns
    }
    fn screen_lines(&self) -> usize {
        self.screen_lines
    }
    fn total_lines(&self) -> usize {
        self.screen_lines
    }
}
#[derive(Clone)]
struct TermProxy(pub UnboundedSender<Event>);
impl EventListener for TermProxy {
    fn send_event(&self, event: Event) {
        // 仅在非 Wakeup 事件时打印，减少噪音
        if let Event::Wakeup = event {
            return;
        }
        // Windows 和 Unix 抛出的事件可能略有不同
        println!("[Event]: {:?}", event);
        self.0.send(event).ok();
    }
}

pub struct Terminal {
    term: Term<TermProxy>,
    pty_info: PtyProcessInfo,
}

impl Terminal {
    pub fn new(config: &Config) -> Self {
        let shell_path;
        if cfg!(target_os = "windows") {
            // 优先使用 PowerShell 7
            shell_path = find_in_env("pwsh.exe");
        } else {
            shell_path = None;
        }
        let shell = shell_path.map(|path| tty::Shell::new(path, vec![]));
        let pty_config = tty::Options {
            shell: shell,
            working_directory: None,
            drain_on_exit: false,
            env: std::collections::HashMap::new(),
            #[cfg(target_os = "windows")]
            escape_args: true,
        };
        let window_size = WindowSize {
            num_lines: 24,
            num_cols: 80,
            cell_width: 1,
            cell_height: 1,
        };
        let pty = tty::new(&pty_config, window_size, 0).expect("创建 PTY 失败");
        let size = TermSize::new(80, 24);
        let (events_tx, events_rx) = tokio::sync::mpsc::unbounded_channel();
        let term = Term::new(config.clone(), &size, TermProxy(events_tx));
        let pty_info = PtyProcessInfo::new(&pty);
        Self { term, pty_info }
    }
}
