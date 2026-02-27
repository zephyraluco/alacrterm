use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty;

// --- 1. 定义终端尺寸 (跨平台通用) ---
struct TermSize {
    columns: usize,
    screen_lines: usize,
}

impl TermSize {
    fn new(columns: usize, screen_lines: usize) -> Self {
        Self { columns, screen_lines }
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

// --- 2. 事件监听器 (跨平台通用) ---
#[derive(Clone)]
struct PrinterProxy;

impl EventListener for PrinterProxy {
    fn send_event(&self, event: Event) {
        // 仅在非 Wakeup 事件时打印，减少噪音
        if let Event::Wakeup = event {
            return;
        }
        // Windows 和 Unix 抛出的事件可能略有不同
        println!("[Event]: {:?}", event);
    }
}

fn main() {
    // 初始化日志
    // env_logger::init();

    // --- 步骤 1: 终端核心初始化 ---
    let config = Config::default();
    let size = TermSize::new(80, 24);
    let proxy = PrinterProxy;

    let term = Term::new(config, &size, proxy.clone());
    let term = Arc::new(FairMutex::new(term));

    // --- 步骤 2: 确定 Shell 和参数 (平台相关) ---
    
    // Windows 下使用 PowerShell，Unix 下读取环境变量或使用 sh
    #[cfg(target_os = "windows")]
    let shell_cmd: &str = "pwsh.exe";
    #[cfg(not(target_os = "windows"))]
    let shell_cmd = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());

    let cmd = tty::Shell::new(shell_cmd.to_string(), vec![]);

    // --- 步骤 3: 配置 PTY 选项 (平台相关) ---
    // Windows 的 Options 结构体多一个 escape_args 字段，使用结构体字面量初始化时需要处理
    let pty_config = tty::Options {
        shell: Some(cmd),
        working_directory: None,
        drain_on_exit: false,
        env: HashMap::new(),
        
        // [Windows 特有] 是否转义参数
        #[cfg(target_os = "windows")]
        escape_args: true,
    };

    let window_size = WindowSize {
        num_lines: 24,
        num_cols: 80,
        cell_width: 1,
        cell_height: 1,
    };

    // --- 步骤 4: 创建 PTY ---
    // tty::new 会根据 target_os 自动调用 windows 或 unix 下的实现
    // 第三个参数 window_id 在无头模式下传 0 即可
    let pty = tty::new(&pty_config, window_size, 0)
        .expect("创建 PTY 失败");

    // --- 步骤 5: 启动事件循环 ---
    let event_loop = EventLoop::new(
        term.clone(),
        proxy,
        pty,
        false, // drain_on_exit
        false, // ref_test
    ).expect("EventLoop 初始化失败");

    // [重要] 在 spawn 之前获取 channel，因为 spawn 会转移所有权
    let sender = event_loop.channel();
    
    // 启动后台 I/O 线程
    let _io_thread = event_loop.spawn();

    println!("终端后端已启动 ({})", if cfg!(windows) { "Windows" } else { "Unix" });

    // --- 步骤 6: 交互演示 (平台相关) ---
    thread::sleep(Duration::from_secs(1)); // 等待 Shell 就绪

    // 根据平台发送不同的命令
    #[cfg(target_os = "windows")]
    let input_bytes = b"dir\r\n"; // Windows 建议 \r\n
    
    #[cfg(not(target_os = "windows"))]
    let input_bytes = b"ls -l\n";

    println!("发送命令...");
    sender.send(Msg::Input(Cow::Owned(input_bytes.to_vec()))).unwrap();

    // 等待执行和回显
    thread::sleep(Duration::from_secs(2));

    // --- 步骤 7: 获取并打印屏幕 ---
    {
        let term_lock = term.lock();
        let grid = term_lock.grid();

        println!("\n--- 终端屏幕快照 ---");
        for i in 0..grid.screen_lines() {
            let line = &grid[Line(i as i32)];
            let mut line_str = String::new();
            let mut has_content = false;

            for j in 0..grid.columns() {
                let cell = &line[Column(j)];
                // 处理空字符，便于显示
                let c = if cell.c == '\0' { ' ' } else { cell.c };
                if c != ' ' { has_content = true; }
                line_str.push(c);
            }

            // 仅打印包含内容的行，或者打印全部以观察布局
            if has_content {
                println!("{}", line_str.trim_end());
            }
        }
        println!("----------------------");
    }
}