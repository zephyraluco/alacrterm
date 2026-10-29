//! 「远端当前目录」的两个来源。
//!
//! 本地会话的 cwd 有本地进程可采样(见 `pty_info`),远端**没有**:想知道远端 shell
//! 现在在哪个目录,只能靠**远端自己说出来**。它一共会说两种话:
//!
//! 1. **OSC 7**(`ESC ] 7 ; file://host/path BEL`)—— 事实标准(iTerm2 / kitty /
//!    VSCode / oh-my-zsh 都发它),精确、带编码。
//! 2. **窗口标题**(OSC 0 / 2)—— 见 [`parse_prompt_title`]。这不是标准,但发行版
//!    自带的 `PS1` 常常就把 cwd 放在里面(Debian / Ubuntu 默认
//!    `PS1=\[\e]0;\u@\h: \w\a\]$PS1` ⇒ 标题 `jz@host: /var/log`),于是**不动远端
//!    一个字节**就能拿到目录。
//!
//! 两条都由 [`DirectoryScanner`] 从字节流里扫出来(见 [`Report`])。
//!
//! ⚠️ **为什么要自己扫**(而不是接模拟器的事件):
//!
//! - OSC 7:`alacritty_terminal`(以及它下面的 `vte`)只认 OSC 0/1/2/4/8/10/11/12/52
//!   这一批,OSC 7 在 `Handler` / `EventListener` 上**没有任何回调**(`vte 0.15` 的
//!   `Handler` 里根本没有 working directory 这一项)。
//! - 标题:模拟器**会**把 OSC 0/2 变成 `TerminalBackendEvent::Title`,但那条事件流
//!   (`TerminalBuilder` 里给 alacritty 监听器的 `events_rx`)在**远端会话下根本没人读**
//!   —— `Terminal::subscribe` 遇到 SSH 就提前返回去跑 `ssh_events`(见那里的注释)。
//!   所以远端标题得跟 OSC 7 一起在字节层认。
//!
//! 反正两条都得在「把字节喂给模拟器**之前**」看一遍(见 `Terminal::feed_remote_bytes`),
//! 一个状态机里一起认最省事。
//!
//! ⚠️ 扫描必须是**跨块**的:远端字节以任意边界到达,一条 OSC 完全可能被切成两块
//! (「`ESC ] 7 ; file`」和「`://host/x BEL`」),所以收状态的活儿得留着。
//!
//! ⚠️ **为什么不直接把 cwd 塞给远端 shell**:我们试过(`export PROMPT_COMMAND=…; exec
//! <shell> -l` 用 `exec` 请求起 shell),但 OpenSSH 只在**无命令的 shell 请求**上才做
//! PAM 会话 —— 换成 `exec` 后 MOTD(`Welcome to Ubuntu …` / 定制横幅)与 `Last login`
//! 全部消失(真机对比验证过)。对远端 shell 动手脚会改变用户的登录环境,代价太大;
//! 所以这里只读不发。想让远端发**精确**的 OSC 7 的用户,自己在 `~/.bashrc` 里加一行
//! (`PROMPT_COMMAND`)即可；应用的自动注入见 `docs/architecture.md`「Shell 集成与 SSH」。

/// 一条「终端现在在哪个目录」的线索。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Report {
    /// 精确:OSC 7,已经是绝对路径。
    Osc7(String),
    /// 近似:窗口标题里的 cwd(见 [`parse_prompt_title`]),可能是 `~` 打头的。
    Title(String),
}

/// 一条 OSC 载荷最多收这么多字节,超了就不再收(见 [`DirectoryScanner::push`])。
///
/// OSC 8(超链接)/ OSC 52(剪贴板)的载荷可以是任意大,而它们**不终止**就得一直收;给个
/// 上限是为了「不认识的长序列不会把内存吃光」。取值远大于任何正常的 OSC 7
/// (`file://` + 一个绝对路径),所以正常上报不会碰到它。
pub(crate) const OSC_PAYLOAD_LIMIT: usize = 64 * 1024;

/// 字节 0x1b(`ESC`,所有控制序列的起点)。
const ESC: u8 = 0x1b;
/// 字节 0x07(`BEL`,OSC 的一种终止符)。
const BEL: u8 = 0x07;

/// 目录扫描器:按字节喂,认出「当前目录」就返回一条 [`Report`]。
///
/// 它只关心 `ESC ] 7 ; …`(OSC 7)与 `ESC ] 0 ; …` / `ESC ] 2 ; …`(窗口标题),
/// 别的控制序列一律**原样跳过** —— 扫的是同一串字节的一份副本,任何时候都不会改写它们
/// (改写是模拟器的活)。
#[derive(Debug, Default)]
pub(crate) struct DirectoryScanner {
    state: State,
    /// 正在收的 OSC 载荷(不含开头的 `ESC ]` 与结尾的终止符)。
    payload: Vec<u8>,
    /// 载荷超限(见 [`OSC_PAYLOAD_LIMIT`]):还在吃字节,但不再留,这条上报只能丢掉。
    oversize: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
enum State {
    /// 字节流里,等一个 `ESC`。
    #[default]
    Ground,
    /// 刚吃到 `ESC`,等 `]`(不是 `]` 就退回 [`State::Ground`])。
    Escape,
    /// 正在收 OSC 载荷,等 `BEL` 或带 ST(`ESC \`)的 `ESC`。
    Osc,
    /// 载荷里遇到 `ESC`,等 ST 的第二个字符 `\`。
    OscEscape,
}

impl DirectoryScanner {
    /// 喂一段字节;返回这一段里**最后一次**认出的线索。
    ///
    /// `None` = 这段字节里没有(或没有一条有效的)线索。一次收集里出现多条时取最后一条:
    /// 字节流是有序的,最后一条才是「现在」。
    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Option<Report> {
        let mut found = None;
        for &byte in bytes {
            match self.state {
                State::Ground => {
                    if byte == ESC {
                        self.state = State::Escape;
                    }
                }
                State::Escape => match byte {
                    b']' => {
                        self.payload.clear();
                        self.oversize = false;
                        self.state = State::Osc;
                    }
                    // `ESC ESC ]`:前一个 `ESC` 不是序列的开头,当前这个才是。
                    ESC => {}
                    _ => self.state = State::Ground,
                },
                State::Osc => match byte {
                    BEL => found = self.finish().or(found),
                    ESC => self.state = State::OscEscape,
                    _ => self.push(byte),
                },
                State::OscEscape => match byte {
                    // `ESC \` = ST:正常的 OSC 结束。
                    b'\\' => found = self.finish().or(found),
                    // 载荷里的裸 `ESC`(有的程序会往标题里塞颜色):按普通字节收着。
                    ESC => {}
                    _ => {
                        self.push(ESC);
                        self.push(byte);
                        self.state = State::Osc;
                    }
                },
            }
        }
        found
    }

    fn push(&mut self, byte: u8) {
        if self.oversize {
            return;
        }
        self.payload.push(byte);
        if self.payload.len() > OSC_PAYLOAD_LIMIT {
            self.payload.clear();
            self.payload.shrink_to_fit();
            self.oversize = true;
        }
    }

    /// 收一条载荷:解析它(若还在收集),并回到 [`State::Ground`]。
    fn finish(&mut self) -> Option<Report> {
        let found = if self.oversize {
            None
        } else {
            parse_report(&self.payload)
        };
        self.payload.clear();
        self.oversize = false;
        self.state = State::Ground;
        found
    }
}

/// 解析一条 OSC 载荷:先看 OSC 7,再把它当窗口标题看(见 [`Report`])。
fn parse_report(payload: &[u8]) -> Option<Report> {
    if let Some(dir) = parse_osc7(payload) {
        return Some(Report::Osc7(dir));
    }
    // 标题(`0;…` / `2;…`,与 vte 分派给 `set_title` 的是同一批)。
    let title = payload
        .strip_prefix(b"0;")
        .or_else(|| payload.strip_prefix(b"2;"))?;
    // ⚠️ trim 与 vte 一致(它会 trim 标题),否则 `\e]0; …` 这类写法会被当成相对路径丢掉。
    parse_prompt_title(String::from_utf8_lossy(title).trim()).map(Report::Title)
}

/// 解析一条 OSC 载荷里的 OSC 7。
///
/// 载荷形如 `7;file://<host><path>`:主机可以为空(`file:///home/x`,有些实现不填主机名),
/// 路径是 URL(可能带 `%XX`)。**宽松**是刻意的 —— 各家 shell 的写法并不完全一致,而我们
/// 要的只是那条路径;判不出来就返回 `None`,不影响字节继续喂给模拟器。
fn parse_osc7(payload: &[u8]) -> Option<String> {
    let url = payload.strip_prefix(b"7;file://")?;
    // 主机到第一个 `/` 为止;没有 `/` 就说明只有主机(还没进任何目录)。
    let path = url.iter().position(|byte| *byte == b'/')?;
    let path = percent_encoding::percent_decode(&url[path..]).decode_utf8_lossy();
    // 只认绝对路径:相对路径对上不了远端文件系统,`~` 之类的展开也不属于这里的职责。
    path.starts_with('/').then(|| path.into_owned())
}

/// 从远端 shell 自己发的**窗口标题**里认当前目录(见模块文档的第 2 条来源)。
///
/// 认两种写法:
///
/// - `jz@host: /var/log`(Debian / Ubuntu 默认 `PS1` 的 `\u@\h: \w`)⇒ 取 `: ` 之后那段;
/// - `/var/log` / `~/proj`(标题就是路径本身)→ 整条都是路径。
///
/// 只认**绝对路径**或 `~` 开头的:相对路径对上不了远端文件系统。`~` 的展开要家目录
/// (`~` → 远端家目录),那在应用层做(文件管理器手里才有 `SshFs`)—— 这里原样留着。
///
/// ⚠️ 这是**启发式**:标题是用户可以随便设的(有人放正在跑的命令名、有人放别的东西),
/// 所以判出来只是「可能」;调用方还要过 `is_dir` 那一关才真跳(见
/// `FilesState::follow_terminal`),所以就算认错也只是不跳,不会乱跳。
pub(crate) fn parse_prompt_title(title: &str) -> Option<String> {
    let tail = match title.split_once(": ") {
        // `@` 那前半段(`jz@host`)不该有空白;带空白的多半是别的东西(`Building: /tmp/out`)。
        Some((prefix, tail)) if !prefix.chars().any(char::is_whitespace) => tail,
        Some(_) => return None,
        None => title,
    };
    let tail = tail.trim();
    let is_path = tail.starts_with('/') || tail == "~" || tail.starts_with("~/");
    is_path.then(|| tail.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 喂一串字节,取出 OSC 7 那条线索(标题那条会被断言排除)。
    fn feed_path(scanner: &mut DirectoryScanner, bytes: &[u8]) -> Option<String> {
        match scanner.feed(bytes) {
            Some(Report::Osc7(dir)) => Some(dir),
            Some(Report::Title(dir)) => panic!("期望 OSC 7,却认出标题 {dir:?}"),
            None => None,
        }
    }

    /// 喂一串字节,取出标题那条线索。
    fn feed_title(scanner: &mut DirectoryScanner, bytes: &[u8]) -> Option<String> {
        match scanner.feed(bytes) {
            Some(Report::Title(dir)) => Some(dir),
            Some(Report::Osc7(dir)) => panic!("期望标题,却认出 OSC 7 {dir:?}"),
            None => None,
        }
    }

    #[test]
    fn bel_terminated_report_is_recognized() {
        let mut scanner = DirectoryScanner::default();
        assert_eq!(
            feed_path(&mut scanner, b"\x1b]7;file://host/home/jz\x07").as_deref(),
            Some("/home/jz")
        );
    }

    #[test]
    fn st_terminated_report_is_recognized() {
        let mut scanner = DirectoryScanner::default();
        assert_eq!(
            feed_path(&mut scanner, b"\x1b]7;file://host/tmp\x1b\\").as_deref(),
            Some("/tmp")
        );
    }

    #[test]
    fn empty_host_is_recognized() {
        let mut scanner = DirectoryScanner::default();
        assert_eq!(
            feed_path(&mut scanner, b"\x1b]7;file:///var/log\x07").as_deref(),
            Some("/var/log")
        );
    }

    #[test]
    fn report_split_across_chunks_is_recognized() {
        let mut scanner = DirectoryScanner::default();
        // 连 `ESC ] 7` 本身都被切开:状态机得扛住任意边界。
        assert_eq!(feed_path(&mut scanner, b"\x1b"), None);
        assert_eq!(feed_path(&mut scanner, b"]7;fil"), None);
        assert_eq!(feed_path(&mut scanner, b"e://host/opt/ros"), None);
        assert_eq!(
            feed_path(&mut scanner, b"\x07").as_deref(),
            Some("/opt/ros")
        );
    }

    #[test]
    fn percent_encoded_path_is_decoded() {
        let mut scanner = DirectoryScanner::default();
        assert_eq!(
            feed_path(&mut scanner, b"\x1b]7;file://host/a%20b/c%2fd\x07").as_deref(),
            Some("/a b/c/d")
        );
        // UTF-8 也能还原(空格与中文都在路径里时,shell 一般会编码)。
        assert_eq!(
            feed_path(&mut scanner, b"\x1b]7;file://host/%E4%B8%AD%E6%96%87\x07").as_deref(),
            Some("/中文")
        );
    }

    #[test]
    fn raw_path_is_kept_as_is() {
        let mut scanner = DirectoryScanner::default();
        // 不编码的实现也认(路径里有空格就直接给空格)。
        assert_eq!(
            feed_path(&mut scanner, b"\x1b]7;file://host/a b\x07").as_deref(),
            Some("/a b")
        );
    }

    #[test]
    fn title_osc_sequences_are_recognized() {
        let mut scanner = DirectoryScanner::default();
        // Debian / Ubuntu 默认 `.bashrc` 的 `\u@\h: \w`(这就是远端实测到的形状)。
        assert_eq!(
            feed_title(&mut scanner, b"\x1b]0;jz@jzrobot-a: /var/log\x07").as_deref(),
            Some("/var/log")
        );
        // `ESC ] 2` 与 `ESC ] 0` 是同一件事(都是窗口标题)。
        assert_eq!(
            feed_title(&mut scanner, b"\x1b]2;jz@host: ~/proj\x1b\\").as_deref(),
            Some("~/proj")
        );
    }

    #[test]
    fn unrelated_osc_sequences_are_skipped() {
        let mut scanner = DirectoryScanner::default();
        // 超链接(OSC 8)、剪贴板(OSC 52)、颜色(OSC 11)都不该被当成目录。
        let bytes =
            b"\x1b]8;;http://example.com\x07\x1b]52;c;Zm9v\x07\x1b]11;rgb:0000/0000/0000\x07";
        assert_eq!(scanner.feed(bytes), None);
        // 而且跳过之后状态要干净:下一条线索还得认出来。
        assert_eq!(
            feed_path(&mut scanner, b"\x1b]7;file://host/srv\x07").as_deref(),
            Some("/srv")
        );
    }

    #[test]
    fn non_path_title_is_skipped() {
        let mut scanner = DirectoryScanner::default();
        // 标题放的是正在跑的命令:不是路径 ⇒ 本次没有线索(但状态要干净)。
        assert_eq!(scanner.feed(b"\x1b]0;vim\x07"), None);
        assert_eq!(
            feed_path(&mut scanner, b"\x1b]7;file://host/x\x07").as_deref(),
            Some("/x")
        );
    }

    #[test]
    fn last_report_in_a_chunk_wins() {
        let mut scanner = DirectoryScanner::default();
        assert_eq!(
            feed_path(
                &mut scanner,
                b"\x1b]7;file://host/a\x07\x1b]7;file://host/b\x07"
            )
            .as_deref(),
            Some("/b")
        );
        // 混着来时也取最后一条(标题紧跟在上报之后 ⇒ 用标题那条)。
        assert_eq!(
            feed_title(
                &mut scanner,
                b"\x1b]7;file://host/a\x07\x1b]0;jz@host: /var/log\x07"
            )
            .as_deref(),
            Some("/var/log")
        );
    }

    #[test]
    fn host_only_report_is_ignored() {
        let mut scanner = DirectoryScanner::default();
        assert_eq!(scanner.feed(b"\x1b]7;file://host\x07"), None);
    }

    #[test]
    fn relative_path_is_ignored() {
        let mut scanner = DirectoryScanner::default();
        assert_eq!(scanner.feed(b"\x1b]7;file://hostrel\x07"), None);
    }

    #[test]
    fn non_file_url_is_ignored() {
        let mut scanner = DirectoryScanner::default();
        assert_eq!(scanner.feed(b"\x1b]7;https://host/x\x07"), None);
    }

    #[test]
    fn oversize_payload_is_dropped_but_state_recovers() {
        let mut scanner = DirectoryScanner::default();
        let mut bytes = b"\x1b]52;c;".to_vec();
        bytes.extend(std::iter::repeat_n(b'A', OSC_PAYLOAD_LIMIT + 16));
        bytes.push(BEL);
        assert_eq!(scanner.feed(&bytes), None);
        assert_eq!(
            feed_path(&mut scanner, b"\x1b]7;file://host/ok\x07").as_deref(),
            Some("/ok")
        );
    }

    #[test]
    fn lone_escape_is_not_a_report() {
        let mut scanner = DirectoryScanner::default();
        assert_eq!(scanner.feed(b"\x1b["), None);
        assert_eq!(
            feed_path(&mut scanner, b"\x1b\x1b]7;file://host/deep\x07").as_deref(),
            Some("/deep")
        );
    }

    #[test]
    fn ubuntu_prompt_title_carries_the_directory() {
        // `\u@\h: \w`:取 `: ` 之后那段;家目录被缩成 `~`(展开是应用层的事)。
        assert_eq!(
            parse_prompt_title("jz@jzrobot-a: /var/log").as_deref(),
            Some("/var/log")
        );
        assert_eq!(parse_prompt_title("jz@jzrobot-a: ~").as_deref(), Some("~"));
        assert_eq!(
            parse_prompt_title("jz@jzrobot-a: ~/my proj").as_deref(),
            Some("~/my proj")
        );
        // vte 会把标题 trim 掉,我们跟它一致(见 `parse_report`)。
        assert_eq!(parse_prompt_title(" /srv ").as_deref(), Some("/srv"));
    }

    #[test]
    fn bare_path_title_is_recognized() {
        assert_eq!(parse_prompt_title("/tmp").as_deref(), Some("/tmp"));
        assert_eq!(parse_prompt_title("~/x").as_deref(), Some("~/x"));
    }

    #[test]
    fn non_path_titles_are_ignored() {
        // 标题放的是当前命令 / 目录名 / 别的东西时,一律不认(宁可不跳,不许乱跳)。
        for title in [
            "vim",
            "bash",
            "~/x: y", // `: ` 之后不是路径
            "正在编译: 30%",
            "Building target: /tmp/out", // 前半段带空白 ⇒ 不像 user@host
            "-",
            "",
        ] {
            assert_eq!(parse_prompt_title(title), None, "{title:?} 不该被当成目录");
        }
    }

    #[test]
    fn path_with_colon_and_space_is_kept() {
        // 路径里本身带 `: ` 时,取第一段 `: ` 之后的部分刚好是完整路径。
        assert_eq!(
            parse_prompt_title("jz@host: /tmp/a: b").as_deref(),
            Some("/tmp/a: b")
        );
    }
}
