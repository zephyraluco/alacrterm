//! 连接参数:连哪儿、以谁的身份、怎么认证、主机密钥怎么验。

use std::fmt;
use std::path::PathBuf;

/// 申请 PTY 时上报的终端类型(`TERM` 的值)。
///
/// 远端 shell / 分页器 / 编辑器都按它决定用什么控制序列,所以必须报一个「支持颜色与
/// 光标控制」的常见值。窗口尺寸由 [`SshSize`] 单独给,这里只管类型。
pub const DEFAULT_TERM: &str = "xterm-256color";

/// 连接一台 SSH 主机所需的全部参数。
#[derive(Clone, Debug)]
pub struct SshParams {
    /// 主机名或 IP(域名会走 DNS 解析)。
    pub host: String,
    /// 端口,通常 22。
    pub port: u16,
    /// 登录用户名。
    pub user: String,
    /// 认证方式(见 [`SshAuth`])。
    pub auth: SshAuth,
    /// 主机密钥校验策略(见 [`HostKeyPolicy`])。
    pub host_key: HostKeyPolicy,
    /// 申请 PTY 时上报的终端类型,默认 [`DEFAULT_TERM`]。
    pub term: String,
    /// 额外交给 [`auth`](crate::auth) 尝试的私钥文件(按顺序,先于默认路径)。
    ///
    /// 空 = 只用默认路径(`~/.ssh/id_ed25519` / `id_ecdsa` / `id_rsa`)。
    pub key_files: Vec<PathBuf>,
    /// 用哪个 `known_hosts` 文件;`None` = `~/.ssh/known_hosts`(与 `ssh` 共用同一份)。
    pub known_hosts: Option<PathBuf>,
}

impl SshParams {
    /// 用最常见的默认值建一份参数:端口 22、免密优先、TOFU 主机密钥、`xterm-256color`。
    pub fn new(host: impl Into<String>, port: u16, user: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            port,
            user: user.into(),
            auth: SshAuth::default(),
            host_key: HostKeyPolicy::default(),
            term: DEFAULT_TERM.to_string(),
            key_files: Vec::new(),
            known_hosts: None,
        }
    }

    /// 端点描述(`user@host:port`):日志、状态栏、错误信息共用。
    pub fn endpoint(&self) -> String {
        format!("{}@{}:{}", self.user, self.host, self.port)
    }

    /// 认证方式:公钥优先,可选追加密码。
    pub fn with_password(mut self, password: impl Into<String>) -> Self {
        self.auth = SshAuth::Password(password.into());
        self
    }
}

/// 认证方式。
///
/// 公钥(agent 与私钥文件)永远先试 —— 这是 OpenSSH 的默认偏好,也让「配好了密钥」
/// 的用户不必每次输密码。
#[derive(Clone, Default)]
pub enum SshAuth {
    /// 只用 ssh-agent 与私钥文件(免密)。
    #[default]
    KeysOnly,
    /// 公钥都失败后再用这个密码试一次。
    Password(String),
}

/// 手写 [`Debug`]:密码绝不能进日志。
impl fmt::Debug for SshAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::KeysOnly => f.write_str("KeysOnly"),
            Self::Password(_) => f.write_str("Password(<隐藏>)"),
        }
    }
}

/// 主机密钥(host key)校验策略。
///
/// 主机密钥是 SSH 唯一的中间人防护:第一次连接必须有个「信任这个指纹」的决定,
/// 之后就靠它不变量来发现冒充。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HostKeyPolicy {
    /// 没见过的主机**先问宿主**(界面弹「是 / 否」),同意后记进 known_hosts
    /// (与 `StrictHostKeyChecking=ask` 同)。默认。
    ///
    /// 没人可问的调用方(如 [`SshFs`](crate::SshFs) 的后台连接)碰到未知主机会直接失败,
    /// 提示先打开该主机的终端会话。
    #[default]
    Ask,
    /// 没见过的**直接信任**并记下(与 `StrictHostKeyChecking=accept-new` 同)。
    AcceptNew,
    /// 只接受 `known_hosts` 里已有的主机,没见过的也拒绝(与 `StrictHostKeyChecking=yes` 同)。
    Strict,
}

/// 远端 PTY 的尺寸。
///
/// 行列是远端真正用来排版的值;像素宽高只给少数图形化程序参考,拿不到就填 0。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SshSize {
    /// 列数(字符)。
    pub cols: u32,
    /// 行数(字符)。
    pub rows: u32,
    /// 单元格宽 × 列数(像素),未知为 0。
    pub pixel_width: u32,
    /// 单元格高 × 行数(像素),未知为 0。
    pub pixel_height: u32,
}

impl SshSize {
    /// 只给行列(像素留 0)。
    pub fn cells(cols: u32, rows: u32) -> Self {
        Self {
            cols,
            rows,
            pixel_width: 0,
            pixel_height: 0,
        }
    }

    /// 行列至少为 1:PTY 尺寸为 0 有些服务端会直接拒绝或把 shell 弄成坏状态。
    pub(crate) fn sanitized(self) -> Self {
        Self {
            cols: self.cols.max(1),
            rows: self.rows.max(1),
            ..self
        }
    }
}
