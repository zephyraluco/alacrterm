//! 平台差异的统一落脚点。
//!
//! 目前只装了 Windows 的一段：PowerShell 的 `cd`(`Set-Location`) 只改 `$PWD`、**不动进程的
//! 当前目录** ⇒ 「跟随终端目录」只能让 shell 自己上报 —— 启动 PowerShell 时注入一段 prompt
//! 包装，把路径塞进 OSC 2（标题）送过来；收到带 [`CWD_PREFIX`] 前缀的标题时 `Terminal`
//! 只更新工作目录、**不改标题**。
//!
//! 整个文件 `#![cfg(windows)]`：非 Windows 上它是空模块，终端启动参数 / 标题处理 /
//! 工作目录来源一律保持原样。以后加其他平台的处理，删掉这一行、在文件内按平台分段即可
//! （调用点的 `#[cfg(windows)]` 不用动）。
//!
//! 机制、代价与实测见 `docs/terminal-architecture.md` §4.5。

#![cfg(windows)]

use std::path::PathBuf;

/// 位置上报的前缀(放在 OSC 2 的标题文本里)。
pub(crate) const CWD_PREFIX: &str = "alacrterm-cwd:";

/// 从一条标题里解出 shell 上报的工作目录(不是我们的上报 ⇒ `None`)。
pub(crate) fn parse_cwd_title(title: &str) -> Option<PathBuf> {
    let path = title.strip_prefix(CWD_PREFIX)?;
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// 启动这个程序时额外追加的参数(空 = 不注入)。
///
/// `-NoExit` 是为了执行完脚本继续进交互;`-EncodedCommand` 传 UTF-16LE + base64,
/// 免得脚本里的引号 / 换行在命令行上被二次解析。
pub(crate) fn integration_args(program: &str) -> Vec<String> {
    if !is_powershell(program) {
        return Vec::new();
    }
    vec![
        "-NoExit".to_string(),
        "-EncodedCommand".to_string(),
        encode_command(POWERSHELL_HOOK),
    ]
}

/// 是不是 PowerShell(按可执行文件名判断,带不带 `.exe` 都认)。
fn is_powershell(program: &str) -> bool {
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    let name = name.to_ascii_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    name == "pwsh" || name == "powershell"
}

/// 注入的 prompt 包装脚本:画提示符前上报 `$PWD`,再调用原来的 prompt(样式不变)。
///
/// `ProviderPath` 只在文件系统 provider 上有值(如 `HKLM:` 就没有)⇒ 取不到时本次不上报。
const POWERSHELL_HOOK: &str = r#"
$global:__alacrterm_prompt = $function:prompt;
function global:prompt {
    $path = $null;
    try { $path = $ExecutionContext.SessionState.Path.CurrentLocation.ProviderPath } catch { };
    if ($path) {
        [Console]::Out.Write([string][char]27 + ']2;alacrterm-cwd:' + $path + [string][char]7);
    }
    if ($global:__alacrterm_prompt) { & $global:__alacrterm_prompt } else { "PS $path> " };
}
"#;

/// `-EncodedCommand` 用的编码:脚本的 UTF-16LE 字节再做 base64(PowerShell 的约定)。
fn encode_command(script: &str) -> String {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64(&bytes)
}

/// 标准字母表的 base64(带 `=`)。手写是为了不给「只此一处用到」的编码加依赖。
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(triple >> 18) as usize & 63] as char);
        out.push(TABLE[(triple >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(triple >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[triple as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_our_own_report_only() {
        assert_eq!(
            parse_cwd_title("alacrterm-cwd:D:\\WorkSpace\\alacrterm"),
            Some(PathBuf::from("D:\\WorkSpace\\alacrterm"))
        );
        assert_eq!(parse_cwd_title("alacrterm-cwd:"), None);
        assert_eq!(parse_cwd_title("pwsh.exe"), None);
        assert_eq!(parse_cwd_title("PS D:\\WorkSpace>"), None);
    }

    #[test]
    fn injects_only_for_powershell() {
        assert!(integration_args("pwsh").len() == 3);
        assert!(integration_args("C:\\Program Files\\PowerShell\\7\\pwsh.exe").len() == 3);
        assert!(integration_args("powershell.exe").len() == 3);
        assert!(integration_args("cmd.exe").is_empty());
        assert!(integration_args("/bin/bash").is_empty());
        assert!(integration_args("ssh").is_empty());
    }

    #[test]
    fn base64_matches_known_values() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn encoded_command_is_utf16le_base64() {
        // `-EncodedCommand` 的约定:UTF-16LE ⇒ base64。"A" = 0x41 0x00 两个字节 ⇒ "QQA="
        assert_eq!(encode_command("A"), "QQA=");
    }
}
