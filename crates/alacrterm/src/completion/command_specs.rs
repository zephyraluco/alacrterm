//! Small offline completion specifications. Never invoke a command to obtain help.
type Spec = (&'static str, &'static str);
const GIT: &[Spec] = &[
    ("status", "查看工作区状态"),
    ("diff", "查看差异"),
    ("log", "查看提交历史"),
    ("add", "暂存文件"),
    ("commit", "创建提交"),
    ("switch", "切换分支"),
    ("branch", "管理分支"),
    ("fetch", "获取远端更新"),
    ("pull", "拉取更新"),
    ("push", "推送提交"),
];
const CARGO: &[Spec] = &[
    ("build", "编译项目"),
    ("check", "检查类型"),
    ("test", "运行测试"),
    ("run", "运行程序"),
    ("clippy", "检查代码问题"),
    ("fmt", "格式化代码"),
    ("doc", "生成文档"),
];
const NPM: &[Spec] = &[
    ("install", "安装依赖"),
    ("run", "运行项目脚本"),
    ("test", "运行测试"),
    ("start", "启动项目"),
    ("ci", "按锁文件安装依赖"),
    ("outdated", "查看过期依赖"),
];
const DOCKER: &[Spec] = &[
    ("ps", "列出容器"),
    ("images", "列出镜像"),
    ("logs", "查看容器日志"),
    ("exec", "在容器中执行命令"),
    ("run", "创建并运行容器"),
    ("build", "构建镜像"),
    ("compose", "管理多容器应用"),
];

pub(super) fn candidates(input: &str) -> Vec<(String, &'static str)> {
    // Deliberately avoid parsing shell operators, quoting or expansion.
    if !input
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || " -_./".contains(c))
    {
        return Vec::new();
    }
    let Some(at) = input.rfind(' ') else {
        return Vec::new();
    };
    let (prefix, partial) = input.split_at(at + 1);
    let words: Vec<_> = prefix.split_whitespace().collect();
    let specs: &[Spec] = match words.as_slice() {
        ["git"] => GIT,
        ["cargo"] => CARGO,
        ["npm"] => NPM,
        ["docker"] => DOCKER,
        ["git", "status"] => &[("--short", "简洁格式"), ("--branch", "显示分支信息")],
        ["git", "diff"] => &[
            ("--staged", "查看暂存差异"),
            ("--stat", "显示变更统计"),
            ("--name-only", "仅显示文件名"),
        ],
        ["git", "log"] => &[
            ("--oneline", "每个提交显示一行"),
            ("--graph", "显示提交关系图"),
            ("--all", "显示所有引用"),
        ],
        ["git", "commit"] => &[("--message", "指定提交消息"), ("--amend", "修改最近提交")],
        ["cargo", "build" | "check" | "test" | "run" | "clippy"] => &[
            ("--release", "使用 release 配置"),
            ("--package", "选择工作区包"),
            ("--features", "启用特性"),
            ("--all-features", "启用所有特性"),
            ("--locked", "要求锁文件保持不变"),
        ],
        ["npm", "install"] => &[("--save-dev", "保存为开发依赖"), ("--global", "全局安装")],
        ["docker", "ps"] => &[("--all", "包括已停止的容器"), ("--quiet", "仅显示容器 ID")],
        ["docker", "logs"] => &[("--follow", "持续查看日志"), ("--tail", "限制末尾日志行数")],
        ["docker", "compose"] => &[
            ("up", "启动服务"),
            ("down", "停止并移除服务"),
            ("logs", "查看服务日志"),
            ("ps", "列出服务"),
        ],
        _ => &[],
    };
    specs
        .iter()
        .filter(|(value, _)| value.starts_with(partial))
        .map(|(value, description)| (format!("{prefix}{value}"), *description))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn specifications_match_context_and_only_insert_literal_suffixes() {
        assert_eq!(
            candidates("git sta"),
            [("git status".into(), "查看工作区状态")]
        );
        assert_eq!(
            candidates("cargo build --rel")[0].0,
            "cargo build --release"
        );
        assert!(candidates("echo git sta").is_empty());
        assert!(candidates("git $(secret) ").is_empty());
        assert!(candidates("git commit message ").is_empty());
        assert_eq!(candidates("git status --short")[0].0, "git status --short");
        assert_eq!(candidates("docker compose u")[0].0, "docker compose up");
    }
}
