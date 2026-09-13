# SSH 远端终端（russh 后端）

> 早先「填了 IP 就用系统 `ssh` 客户端」的做法只能算把远端终端**委托**出去了：密码填了也没用
> （`ssh` 刻意不接受命令行传密码）、连接状态无从得知、`conpty.dll` 那一套 Windows 兜底也只对
> 本地 PTY 生效。本文记录改成 `russh` 直连之后的实现。
>
> 相关文件：`crates/terminal/src/ssh.rs`（后端，含单元测试）、`crates/terminal/src/alacritty.rs`
> （后端抽象）、`crates/terminal/src/terminal.rs`（装配 / 事件）、`crates/alacrterm/src/connection_dialog.rs`
> （表单 → 建连请求）、`crates/alacrterm/src/host_key_dialog.rs`（主机密钥确认对话框）、
> `crates/alacrterm/src/settings_window.rs`（校验策略设置）。

---

## 1. 核心结论：终端仿真完全不知道自己在跑远端

SSH 后端**没有**引入第二套终端模型，也没有让渲染层知道「这是远端」。它只是换了
「字节从哪来、往哪去」：

| | 本地 PTY | SSH 远端 |
|---|---|---|
| 上行（→ 网格） | `alacritty_terminal` 的 `EventLoop` IO 线程读 PTY，用 `Processor::advance` 写 `Term` | SSH 线程读 `ChannelReadHalf`，用同一个 `Processor::advance` 写同一个 `Term` |
| 下行（网格 →） | `Notifier(Msg::Input/Resize/Shutdown)` | `SshInput` 通过无界通道送给 SSH 线程，再 `ChannelWriteHalf::data_bytes / window_change / eof` |
| 唤醒 UI | `TerminalBackendEvent::Wakeup` | 同一个 `Wakeup` |
| 进程信息 | PTY 子进程的 pid（`PtyProcessInfo`） | 无（`ProcessIdGetter::none()`） |
| 会话结束 | PTY drain 后 `Exit` / `ChildExit` | 通道 `Eof`/`Close` 后自行发 `Exit` |

所以终端仿真、渲染、输入法、选择、搜索、超链接等**全部原样复用**；差异被压缩在一个 trait 里。

---

## 2. 并发模型：一个专属 OS 线程 + 自己的 tokio 运行时

`russh` 要求 `tokio::io::AsyncRead/AsyncWrite`，而 gpui 的后台执行器不是 tokio 运行时
（混用会把会话绑死在错误的任务上）。因此 `ssh::spawn` 起一个名为 `alacrterm-ssh` 的
**专属线程**，线程内建一个 `current_thread` 运行时并 `block_on` 整个会话：

```
gpui 后台任务 ──spawn()──> std::thread "alacrterm-ssh"
   │                          └─ tokio current_thread runtime
   │                               ├─ client::connect / authenticate / channel_open_session
   │                               └─ select!{ 命令 / 远端数据 / keepalive }
   │
   ├─ SshInput（futures 无界通道 Sender）→ 线程
   └─ term(Arc<FairMutex<Term>>) + events_tx ← 线程（直接写网格 + 发 Wakeup）
```

两侧只通过 `futures::channel::mpsc` 的 Sender（gpui 侧）与 `Arc<FairMutex<Term>>`（共享内存）
通信——都是纯发送，不要求两端运行时互通。

**为什么 `spawn` 立即返回**：连接是异步的，连不上**不会**让 `TerminalBuilder::new_ssh` 失败。
失败原因会作为文本写进终端网格（见 §5），会话以「已断开」状态留在标签页里——比弹一个错误页
更符合终端的习惯（远端报的原因不会被丢掉）。

---

## 3. 连接流程

```
client::connect(config, (host, port), handler)   ← 20s 超时
    └─ handler.check_server_key()                ← 主机密钥校验（按策略，可能弹窗）
authenticate()                                   ← 密码 → 免密 → ~/.ssh 私钥
handle.channel_open_session()
channel.split() → (reader, writer)               ← 拆半，见 §4
writer.request_pty(true, "xterm-256color", 80, 24, 0, 0, &[])
writer.request_shell(true)
```

### 3.1 主机密钥：按 `StrictHostKeyChecking` 策略校验

`ClientHandler::check_server_key` 用 `russh::keys::known_hosts` 判断出「已记录且一致 / 没有记录 /
记录不一致」，再按 `SshOptions::host_key_checking`（默认 `Ask`）决定怎么处理：

| 策略 | 没有记录（首次连接） | 记录不一致（主机重装过，或可能被冒充） |
|---|---|---|
| `Ask`（默认） | 弹窗显示指纹，用户选「信任并继续」→ 写入 known_hosts | 弹窗对比新旧指纹，选信任 → **替换**旧记录 |
| `AcceptNew` | 直接写入并接受（改动前的 TOFU 行为） | 拒绝 |
| `Yes` | 拒绝 | 拒绝 |
| `No` | 直接写入并接受 | 直接替换（⚠️ 不安全） |

- 密钥**一致时永远不弹窗**（绝大多数连接都走这条快路）；
- 定位不到文件（没有 home 目录）→ 跳过校验并告警（与 OpenSSH「没有 known_hosts 就当首次连接」一致）；
- 与 OpenSSH 的唯一差别：`Ask` 在**密钥变更**时也允许用户选择替换记录；OpenSSH 的 `ask` 只能停下
  让人手工改 known_hosts（`accept-new` / `yes` 也只会拒绝）。这是有意的——否则重装过的机器在界面上
  完全没有出路；
- 替换由 `forget_known_host` 自己重写文件：22 端口存 `host`、其余存 `[host]:port`，一行的主机字段
  可以是逗号分隔的多个名字，同一主机的**所有**记录行都会删掉（其它算法的旧记录同样不可信了）。
  ⚠️ **不能**用 russh 的 `known_host_keys_path` 给的行号：它统计行数时会跳过 `#` 注释行
  （`continue` 跳过了 `line += 1`），文件里有注释就会整体错位；
- ⚠️ `HashKnownHosts` 写出的 `|1|salt|hash` 条目回推不出主机名，匹配不到（返回 0 行）→ 这时会
  **拒绝**连接并让用户手工清理，而不是假装替换成功。

### 3.2 认证顺序

1. **填了密码 → 只用密码**，失败即报错（不再偷偷试私钥，行为可预期）；
2. 没填密码：先 `authenticate_none`（部分服务器/已配好免密的环境直接放行）；
3. 再依次尝试 `~/.ssh/id_ed25519` → `id_ecdsa` → `id_rsa`（与 OpenSSH 默认顺序一致）；
4. 都用不了就报出具体原因（`认证失败：未填写密码，且 ~/.ssh 下没有可用的私钥…`）。

RSA 私钥要先和服务器协商签名哈希（`best_supported_rsa_hash`），非 RSA 必须传 `None`——
传错 `PrivateKeyWithHashAlg` 会让握手失败。

### 3.3 初始 PTY 尺寸为什么是 80×24

终端实体在任何一次布局之前用的是 `TerminalBounds::default()`（100 列 × 6 行）。
拿它去开远端 PTY 会让 shell 先看到一个 6 行的窗口，而某些 shell / readline 在极矮窗口下会
重排甚至清屏（这正是本地 PTY 那边 `conpty.dll` 系列问题的同一类症状）。
所以先按惯例报 **80×24**，紧接着的第一次布局就会用真实尺寸 `window_change` 覆盖它。

---

### 3.4 交互式确认的往返路径

`check_server_key` 在**握手路径**上：拿不到用户的回答就不能继续握手，所以需要一条
「SSH 线程 → UI → SSH 线程」的往返：

```
alacrterm-ssh 线程                                   gpui（主线程）
  check_server_key
    └─ HostKeyPrompt{端点/算法/指纹/旧指纹, Sender} ─┐
       等待 answers.next()（最长 HOST_KEY_TIMEOUT）  │
       ↑                                            ↓
       └──── HostKeyDecision ──── HostKeyPrompt::respond() ←── 对话框按钮
```

- 请求经 `TerminalBackendEvent::HostKeyPrompt` → `Terminal::process_event` 转成
  `terminal_view::Event::HostKeyPrompt` → 应用层提供的 `HostKeyPromptHandler`
  （`terminal_panel::host_key_prompt_handler`）→ `AppRoot::defer_after_update` → 弹对话框
  （`host_key_dialog.rs`）。事件回调里只有 `&mut App`（没有窗口），所以必须让出一拍再开对话框；
- 回答只认第一次：`HostKeyPrompt::respond` 把发送端从 `Mutex<Option<_>>` 里 `take()` 出来，
  确认 / 取消 / 关闭（`on_ok` / `on_cancel` / `on_close`）三条路径重复回答也不会串；
- 等不到回答一律按**拒绝**处理：对话框被关掉或应用退出 → 发送端被丢弃 → 接收端读到 `None`；
  超过 `HOST_KEY_TIMEOUT`（120s）→ 超时；
- ⚠️ 这段等待在 `client::connect` **内部**，所以外层建连超时是
  `CONNECT_TIMEOUT + HOST_KEY_TIMEOUT`（只在 `Ask` 时加）：否则用户还在比指纹，连接就先超时了。
  代价是 `Ask` 策略下连不通的主机最长要等 140s（SYN 被黑洞的情况）；
- UI 侧用**窗口级**对话框而不是终端区浮层：连接是在后台完成的，发起连接的会话未必是当前标签页，
  浮层会没人看到。

`HostKeyPrompt` 手写了 `Debug` / `PartialEq`（按发送端 `Arc::ptr_eq` 比较）：`terminal::Event` 要求
`Clone + Debug + PartialEq + Eq`，而 `UnboundedSender` 不满足后两者。

---

## 4. 通道必须拆成读写两半

`Channel::wait(&mut self)` 借的是 `&mut`，而 `data_bytes` 等写方法是 `&self`。若在
`tokio::select!` 里同时对同一个 `Channel` 等待读与写，借用会打架（select 的 future 在
handler 执行期间仍然存活）。`russh` 为此提供了 `Channel::split() -> (ChannelReadHalf, ChannelWriteHalf)`：

```rust
let (mut reader, writer) = channel.split();
loop {
    tokio::select! {
        biased;
        command = commands.next() => { /* writer.data_bytes / window_change / eof */ }
        message = reader.wait()    => { /* Processor::advance + Wakeup */ }
        _ = keepalive.tick()       => { /* handle.send_keepalive(true) */ }
    }
}
```

三个分支各借一个互不相干的对象（`commands` / `reader` / `keepalive`），handler 里可以随意用
`writer` 与 `handle`，不需要任何 `Arc<Mutex<Channel>>` 之类的绕路。

**保活**：每 30s 发一次 keepalive（`Handle::send_keepalive`），并顺带检查 `Handle::is_closed()`。
不用 `client::Config::keepalive_interval` 是因为要避免绑定到该结构体某个具体版本的字段名；
自己发更直观，效果一样（拔网线这种静默断开最终会报错而不是永远挂着）。

**标签页关闭 / 应用退出**：`Terminal::Drop` → `PtySender::shutdown()` → 送 `Command::Shutdown`
→ 线程 `eof()` + `close()` 后退出。因为用的是 `select!`，即使正卡在 `reader.wait()` 上也能被唤醒，
不会留下一个永久挂着的线程（`Term` 的 `Arc` 也随之释放）。

---

## 5. 状态、提示与 UI 反馈

### 5.1 网格内的本机提示（`write_notice`）

SSH 后端没有可写的本地进程，提示只能自己生成 ANSI 序列喂给 `Processor`：

| 时点 | 内容 |
|---|---|
| 开始连接 | `\x1b[90m正在连接 user@host:port …\x1b[0m`（**不带换行**） |
| 连通 | `\r\x1b[2K` 原地擦掉上一行，让位给远端首屏输出 |
| 失败 | `\r\n\x1b[31m连接中断：<原因>\x1b[0m\r\n` |
| 会话结束 | `\r\n\x1b[90m连接已断开（远端退出码 N）\x1b[0m\r\n` |

⚠️ 换行一律写 `\r\n`：仿真器默认不把单独的 `\n` 当作回车换行（与 `Terminal::write_output`
里那段「给 LF 补 CR」是同一个道理）。

用户没信任主机密钥时，russh 只报一句英文 `Unknown server key`；`run()` 里把它换成了
「连接中断：主机密钥未被信任，已放弃连接」，网格与日志都能看懂。

### 5.2 会话状态：`TerminalBackendEvent::Connected` → 状态栏

远端 shell 不在本机进程表里，`Terminal::pid()` 拿不到，`status_metrics` 的
「进程是否存在」三态判断对 SSH 会话永远停在「启动中」。因此新增了一条极轻的状态通路：

```
ssh.rs 通道就绪 ──UnboundedSender──> TerminalBackendEvent::Connected
   └─ Terminal::process_event  → self.connected = true
        └─ TerminalView::is_connected(cx)（读实体，不额外发事件）
             └─ status_bar：SSH 会话按 (exited, connected) → 已断开 / 运行中 / 连接中
```

本地会话的 `connected` 一建出来就是 `true`（PTY 已经在跑），状态栏仍走原来
「事件 + 采样」的判断，两套逻辑互不干扰。

### 5.3 会话显示名

用户没填「名称」时，SSH 会话用 `user@host`（`SessionRequest::Ssh` 在 `spawn_session` 里补齐），
而不是等终端的 OSC 标题——远端不一定上报标题，而标签页 / 侧边栏 / 状态栏都需要一个稳定标识。
本地会话维持原样（回退到终端标题）。

### 5.4 用户名与端口

- 用户名为空 → 取本机 `USERNAME` / `USER`（与 OpenSSH 的默认行为一致），都没有则 `root`；
- 端口为空 / 非数字 / 0 → 回退 22 并打 `warn` 日志（SSH 端口没有合理的自动纠正手段，
  静默回退比拒绝建连更符合预期）。

---

## 6. 后端抽象（`alacritty.rs`）

```rust
/// 终端后端的「下行」通道
pub(super) trait TerminalInput: Send + 'static {
    fn write(&self, input: Cow<'static, [u8]>);
    fn resize(&self, size: WindowSize);
    fn shutdown(&self);
}

pub(super) struct PtySender { input: Box<dyn TerminalInput> }
```

- `LocalInput { notifier: Notifier }`：投递到 alacritty 事件循环（原来的行为）；
- `SshInput { commands: UnboundedSender<Command> }`：送给 SSH 线程。

`Terminal` 对这两种后端**零感知**：它只会 `pty_tx.notify/resize/shutdown`。
`TerminalBuilder` 侧则用一个私有枚举分叉（`Transport::Local` / `Transport::Ssh`），
两支各自产出 `Backend { pty_tx, info, title_override, template_shell }` 再合流到同一个
`Terminal` 字面量——上游那段建终端流程因此只**分叉一次**，不需要复制。

`pty_info.rs` 为此增加了 `PidSource::None`（`ProcessIdGetter::none()`）：
- `pid()` 返回 `None` → 状态栏 CPU / 内存显示 `--`，标题不用本机进程名；
- ⚠️ `terminate_child_process`（unix）必须先判 `is_local()`：`fallback_pid()` 为 0 时
  `killpg(0, SIGTERM)` 会**作用到本进程所在的整个进程组**（等于自杀）。

---

## 7. 依赖与构建

```toml
russh = { version = "0.63", default-features = false, features = ["ring", "rsa", "flate2"] }
tokio = { version = "1", features = ["rt", "net", "time", "io-util", "macros"] }
```

⚠️ **必须关掉默认的 `aws-lc-rs` 加密后端**：`aws-lc-sys` 在 Windows 上要求 NASM，本机没有
（`NASM command not found! Build cannot continue.`，`aws-lc-sys` 的 build script 直接 panic）。
`ring` 自带预生成的汇编，无需额外工具链；`rsa`（纯 Rust）保留以兼容 RSA 主机密钥/客户端密钥，
`flate2` 是纯 Rust 的 zlib 后端，用于可选的 SSH 压缩。

`SshOptions` 用 `SshOptions::new(host, port, user, password)` 构造（字段私有，避免密码被
随手 `Debug` 打印——`Debug` 是手写的，密码只显示「已设置」）。
`with_known_hosts(path)` 对应 OpenSSH 的 `UserKnownHostsFile`，测试用它避免污染真实用户的
`~/.ssh/known_hosts`；`with_host_key_checking(policy)` 对应 `StrictHostKeyChecking`（默认 `Ask`，
应用里由设置窗口的开关切换 `Ask` / `AcceptNew`）。

---

## 8. 测试（`crates/terminal/src/ssh.rs` 的 `#[cfg(test)] mod tests`）

这些用例直接调 `ClientHandler::check_server_key`（用 `futures::join!` 同时在另一支里回答询问），
不需要真服务端：

| 用例 | 覆盖 |
|---|---|
| `forget_known_host_*`（4 个） | 替换记录时只删这台主机这一端口的行；认逗号分隔的主机名；哈希条目不动；没有记录时原样返回 |
| `ask_policy_prompts_and_remembers_on_trust` | `Ask` + 未知主机：询问带端点 / 算法 / `SHA256:` 指纹 / `Unknown`，信任后写入 known_hosts |
| `ask_policy_rejects_on_decline` | `Ask` + 取消 → 拒绝且不写记录 |
| `ask_policy_replaces_changed_key_on_trust` | 密钥不一致：询问带 `Changed{known}`，信任后旧记录被删、新记录写入，别的主机不受影响 |
| `accept_new_policy_never_prompts` | `AcceptNew` 不弹窗，直接写入 |
| `accept_new_policy_rejects_changed_key` | 密钥不一致 + 非 `Ask`：直接拒绝且不改文件 |
| `yes_policy_rejects_unknown_host` | `Yes` 下未知主机直接拒绝 |

```powershell
cargo test -p terminal --lib ssh::
```

⚠️ 每个用例用**独立的**临时文件名：cargo 默认并行跑测试，共用文件名会互相覆盖。
⚠️ `cargo test -p terminal` 里有 23 个 `alacritty::hyperlinks::tests::path::*` 是**既有失败**
（与本改动无关）。

原先那套「真起一个 russh 服务端」的端到端测试（`ssh_tests.rs`）已不在仓库里；需要端到端时走 §9.1。

---

## 9. 手工验收

1. 启动 `cargo run -p alacrterm`，点标签栏「+」或侧边栏右键「新建终端」；
2. 填 IP（如 `192.168.1.10`）、端口、用户名、密码 → 「连接」；
3. 预期：终端区先出现灰色「正在连接 user@host:22 …」，连通后该行消失、出现远端 shell 提示符；
   状态栏显示 `● 运行中 · 连接 SSH user@host:22`，CPU / 内存为 `--`（远端进程本机采不到）；
4. 故意填错密码：终端网格出现红色「连接中断：密码认证被拒绝」，状态栏变为「已断开」；
5. 远端 `exit`：网格追加灰色「连接已断开（远端退出码 0）」，标签与内容保留（不关标签、不退出应用）。

### 9.1 主机密钥确认

需要一个「能连上、但主机密钥由我们掌控」的服务端。最小做法是临时写一个 russh 服务端：
`auth_none` 直接接受，`pty_request` / `shell_request` 里调 `session.channel_success(channel)`
（否则客户端一直在等应答），主机密钥用
`ssh-keygen -t ed25519 -N '' -f target/fake_key` 生成后 `load_secret_key` 读入，监听 127.0.0.1。
换一把私钥重启就能复现「密钥已变更」。

1. 连 `127.0.0.1`（首次）→ 弹「首次连接这台主机」（主机 / 算法 / `SHA256:…` 指纹）；点「信任并继续」
   → 会话建立、`~/.ssh/known_hosts` 多一行 `127.0.0.1 ssh-ed25519 AAAA…`；
2. 换主机私钥重启服务端再连 → 弹「主机密钥已变更」，并列出「已记录」的旧指纹；点「信任并继续」
   → 那是**替换**（文件里仍只一行）而不是追加；
3. 换一个没记录过的主机名（如 `localhost`）→ 弹窗后点「取消」→ 网格出现红色
   「连接中断：主机密钥未被信任，已放弃连接」，known_hosts 不留记录；
4. 设置窗口 → SSH → 关掉「新主机询问是否信任」→ 再连 `localhost` → **不弹窗**，直接写入并连上。

⚠️ 自动化这类界面时的要点（脚本是临时的，放 `target/`、不入库）：坐标用**物理像素**，并把
pwsh 进程设成 DPI-aware，这样 `GetWindowRect` / `SetCursorPos` / 截图像素三者坐标系一致；
点击前必须把窗口置为前台（否则第一次点击只用来激活，按钮收不到）；喂文字用
`PostMessage(WM_CHAR)`、按键用 `PostMessage(WM_KEYDOWN/UP)`（`KEYUP` 的 lParam 要带 bit30 + bit31，
否则 `TranslateMessage` 会把它当按下、再生成一条 `WM_CHAR`）；截图前先 `ShowWindow` +
`SetWindowPos(TOPMOST)`，gpui 窗口被遮住时不重绘、`PrintWindow` 会拿到上一帧。

---

## 10. 已知边界（有意未做）

- 不解析 `~/.ssh/config`：Host 别名、`ProxyJump` / `ProxyCommand`、`IdentityFile`、`Port` 等
  （用户得把真实的 host / port / 用户填进对话框）；
- 不支持**加密私钥**（需要口令输入，界面没有这个入口）与 **ssh-agent**（`SSH_AUTH_SOCK` /
  Pageant / Windows OpenSSH agent）；
- 主机密钥校验只有弹窗询问（`Ask`）/ 自动信任（`AcceptNew`）/ 只信记录（`Yes`）/ 一律接受（`No`）
  四种策略，且是**全局**设置（设置窗口 → SSH），不能按主机配置，也没有持久化（每次启动回到 `Ask`）；
  非交互的 `Yes` / 危险的全接受 `No` 没有暴露到界面上；
- `HashKnownHosts` 写出的 `|1|…` 条目无法识别：替换时删不掉，只能拒绝连接并让用户手工清理（见 §3.1）；
- 不转发 X11 / agent / 端口；
- 不支持键盘交互式（keyboard-interactive）认证——只做 password / none / publickey。

以上若要补，落点都在 `crates/terminal/src/ssh.rs` 的 `authenticate` 与 `ClientHandler`
（外加对话框字段），不会波及终端仿真与渲染。
