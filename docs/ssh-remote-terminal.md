# SSH 远端终端（russh 后端）

> 早先「填了 IP 就用系统 `ssh` 客户端」的做法只能算把远端终端**委托**出去了：密码填了也没用
> （`ssh` 刻意不接受命令行传密码）、连接状态无从得知、`conpty.dll` 那一套 Windows 兜底也只对
> 本地 PTY 生效。本文记录改成 `russh` 直连之后的实现。
>
> 相关文件：`crates/terminal/src/ssh.rs`（后端）、`crates/terminal/src/ssh_tests.rs`（端到端测试）、
> `crates/terminal/src/alacritty.rs`（后端抽象）、`crates/terminal/src/terminal.rs`（装配）、
> `crates/alacrterm/src/connection_dialog.rs`（表单 → 建连请求）。

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
    └─ handler.check_server_key()                ← 主机密钥校验（TOFU）
authenticate()                                   ← 密码 → 免密 → ~/.ssh 私钥
handle.channel_open_session()
channel.split() → (reader, writer)               ← 拆半，见 §4
writer.request_pty(true, "xterm-256color", 80, 24, 0, 0, &[])
writer.request_shell(true)
```

### 3.1 主机密钥：TOFU（首次使用即信任）

`ClientHandler::check_server_key` 用 `russh::keys::known_hosts`：

- 记录存在且一致 → 接受；
- **没有记录** → `learn_known_hosts_path` 写入 known_hosts 后接受（等价于 `ssh` 首次连接回答 `yes`）；
- 记录存在但**不一致** → 拒绝并记 error 日志（可能被中间人替换）；
- 定位不到文件（没有 home 目录）→ 跳过校验并告警（与 OpenSSH「没有 known_hosts 就当首次连接」一致）。

已知限制：**没有** `ssh` 那种「unknown host，是否继续？(yes/no)」的交互确认，也没有
`StrictHostKeyChecking` / `UserKnownHostsFile` 的配置文件支持（后者有 API 级的等价物，
见 §7 的 `SshOptions::with_known_hosts`）。

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
`~/.ssh/known_hosts`。

---

## 8. 测试（`crates/terminal/src/ssh_tests.rs`）

四个用例都在 127.0.0.1 上真起一个最小 `russh` 服务端（密码认证 + 会话通道 + PTY/shell + 回显），
因为这条链路的正确性恰恰在协议交互里，mock 掉就什么都证明不了：

| 用例 | 覆盖 |
|---|---|
| `connects_and_streams_data_both_ways` | 认证 → PTY/shell 应答 → 远端 banner 进网格 → 本机输入送达远端并回显 |
| `host_key_is_remembered_after_first_connect` | 首次连接写入 known_hosts（`[127.0.0.1]:port`），第二次命中记录仍能连上 |
| `wrong_password_is_reported_in_grid` | 认证失败原因如实写进网格 |
| `unreachable_host_is_reported_in_grid` | 连不上时网格里给出原因，而不是静默留白 |

关键点/坑：

- 服务端**必须自带主机密钥**（`server::Config::keys`），否则客户端直接以
  `No common Key algorithm - ours: [..], theirs: []` 失败。测试内嵌了一把一次性 ed25519
  私钥（`ssh-keygen -t ed25519 -N ''` 生成，仅测试用）：运行时生成需要 `rand`，而
  `PrivateKey::random` 的 `CryptoRng` 约束来自 ssh-key 内部的 `rand_core` 版本，
  外部 `rand` 的版本对不上就会编译失败（0.9 的 `ThreadRng` 不满足）。
- 服务端 `Session::channel_success(channel)` 是**同步**方法（不是 async）；客户端
  `request_pty(true, ..)` / `request_shell(true)` 会等这个应答，不回应就是永久挂起。
- 等待用 `tokio::time::sleep`，**不能**用 `std::thread::sleep`：测试与假服务端跑在同一个
  current-thread 运行时上，阻塞式 sleep 会把服务端一起饿死。
- 每个用例一个独立的临时 known_hosts（含 pid 与用例名），跑完删除。

```powershell
cargo test -p terminal ssh_tests -- --test-threads=1
```

⚠️ `cargo test -p terminal` 里有 23 个 `alacritty::hyperlinks::tests::path::*` 是**既有失败**
（与本改动无关）。

---

## 9. 手工验收

1. 启动 `cargo run -p alacrterm`，点标签栏「+」或侧边栏右键「新建终端」；
2. 填 IP（如 `192.168.1.10`）、端口、用户名、密码 → 「连接」；
3. 预期：终端区先出现灰色「正在连接 user@host:22 …」，连通后该行消失、出现远端 shell 提示符；
   状态栏显示 `● 运行中 · 连接 SSH user@host:22`，CPU / 内存为 `--`（远端进程本机采不到）；
4. 故意填错密码：终端网格出现红色「连接中断：密码认证被拒绝」，状态栏变为「已断开」；
5. 远端 `exit`：网格追加灰色「连接已断开（远端退出码 0）」，标签与内容保留（不关标签、不退出应用）。

---

## 10. 已知边界（有意未做）

- 不解析 `~/.ssh/config`：Host 别名、`ProxyJump` / `ProxyCommand`、`IdentityFile`、`Port` 等
  （用户得把真实的 host / port / 用户填进对话框）；
- 不支持**加密私钥**（需要口令输入，界面没有这个入口）与 **ssh-agent**（`SSH_AUTH_SOCK` /
  Pageant / Windows OpenSSH agent）；
- 没有 `ssh` 的交互式主机密钥确认，也没有 `StrictHostKeyChecking=no` 之类的开关；
- 不转发 X11 / agent / 端口；
- 不支持键盘交互式（keyboard-interactive）认证——只做 password / none / publickey。

以上若要补，落点都在 `crates/terminal/src/ssh.rs` 的 `authenticate` 与 `ClientHandler`
（外加对话框字段），不会波及终端仿真与渲染。
