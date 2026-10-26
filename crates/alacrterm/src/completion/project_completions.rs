//! Project metadata providers: no subprocesses, package scripts, hooks or generators.
use super::{
    completion::{Candidate, Kind, Source},
    path_suggestions::PathQuery,
};
use std::{collections::BTreeSet, path::Path};
const LIMIT: usize = 256 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Provider {
    NpmScripts,
    GitBranches,
}
#[derive(Clone, Debug)]
pub(super) struct Query {
    pub provider: Provider,
    pub path: PathQuery,
}
impl Query {
    pub fn parse(path: &PathQuery) -> Option<Self> {
        let words: Vec<_> = path
            .input
            .get(..path.replacement.start)?
            .split_whitespace()
            .collect();
        let provider = match words.as_slice() {
            ["npm", "run" | "run-script"] => Provider::NpmScripts,
            ["git", "switch" | "checkout" | "merge" | "rebase"] => Provider::GitBranches,
            _ => return None,
        };
        Some(Self {
            provider,
            path: path.clone(),
        })
    }
    pub fn command(&self) -> &'static str {
        match self.provider {
            Provider::NpmScripts => "npm",
            Provider::GitBranches => "git",
        }
    }
    pub fn candidates(&self, values: Vec<String>) -> Vec<Candidate> {
        let prefix = format!("{}{}", self.path.directory, self.path.leaf);
        values
            .into_iter()
            .filter(|name| name.starts_with(&prefix) && *name != prefix && literal_name(name))
            .map(|name| {
                let mut item = Candidate::new(name, 0, Kind::Argument, Source::Project, 1300);
                item.replacement = self.path.replacement.clone();
                item.description = Some(
                    match self.provider {
                        Provider::NpmScripts => "package.json 脚本",
                        Provider::GitBranches => "Git 分支",
                    }
                    .into(),
                );
                item
            })
            .collect()
    }
}
fn literal_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "_./:+-".contains(ch))
}
fn scripts(data: &[u8]) -> Vec<String> {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(data) else {
        return Vec::new();
    };
    value
        .get("scripts")
        .and_then(|value| value.as_object())
        .map(|scripts| {
            scripts
                .iter()
                .filter(|(_, command)| command.is_string())
                .map(|(name, _)| name.clone())
                .collect()
        })
        .unwrap_or_default()
}
fn packed(data: &[u8], values: &mut BTreeSet<String>) {
    for line in String::from_utf8_lossy(data).lines() {
        if let Some((_, name)) = line.split_once(' ') {
            if let Some(name) = name
                .strip_prefix("refs/heads/")
                .or_else(|| name.strip_prefix("refs/remotes/"))
            {
                if !name.ends_with("/HEAD") && literal_name(name) {
                    values.insert(name.into());
                }
            }
        }
    }
}
fn read(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read as _;
    if !std::fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    let mut data = Vec::new();
    file.take(LIMIT as u64 + 1).read_to_end(&mut data).ok()?;
    (data.len() <= LIMIT).then_some(data)
}
pub(super) fn local(provider: Provider, cwd: &Path) -> Vec<String> {
    if provider == Provider::NpmScripts {
        return read(&cwd.join("package.json")).map_or(Vec::new(), |data| scripts(&data));
    }
    let Some(root) = cwd
        .ancestors()
        .take(32)
        .find(|root| root.join(".git").exists())
    else {
        return Vec::new();
    };
    let git = root.join(".git");
    let mut git = if git.is_file() {
        let Some(data) = read(&git) else {
            return Vec::new();
        };
        let text = String::from_utf8_lossy(&data);
        let Some(path) = text.trim().strip_prefix("gitdir: ") else {
            return Vec::new();
        };
        root.join(path)
    } else {
        git
    };
    if let Some(data) = read(&git.join("commondir")) {
        git = git.join(String::from_utf8_lossy(&data).trim());
    }
    let mut values = BTreeSet::new();
    if let Some(data) = read(&git.join("packed-refs")) {
        packed(&data, &mut values);
    }
    let mut queue = vec![
        (git.join("refs/heads"), String::new()),
        (git.join("refs/remotes"), String::new()),
    ];
    let mut visited = 0;
    while let Some((directory, prefix)) = queue.pop() {
        if visited >= 4096 {
            break;
        }
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.take(4096 - visited).flatten() {
            visited += 1;
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let name = format!("{prefix}{name}");
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                if name.matches('/').count() < 16 {
                    queue.push((entry.path(), format!("{name}/")));
                }
            } else if name != "HEAD" && !name.ends_with("/HEAD") && literal_name(&name) {
                values.insert(name);
            }
        }
    }
    values.into_iter().take(4096).collect()
}
pub(super) async fn remote(provider: Provider, cwd: &str, fs: &terminal::SshFs) -> Vec<String> {
    if !cwd.starts_with('/') {
        return Vec::new();
    }
    if provider == Provider::NpmScripts {
        return fs
            .read_completion_file(format!("{cwd}/package.json"))
            .await
            .map_or(Vec::new(), |data| scripts(&data));
    }
    let mut root = cwd.trim_end_matches('/').to_string();
    let mut git = None;
    for _ in 0..8 {
        let candidate = format!("{root}/.git");
        if let Ok(data) = fs.read_completion_file(candidate.clone()).await {
            let text = String::from_utf8_lossy(&data);
            if let Some(path) = text.trim().strip_prefix("gitdir: ") {
                git = Some(join_remote(&root, path));
                break;
            }
        }
        if fs.list_completion_dir(candidate.clone()).await.is_ok() {
            git = Some(candidate);
            break;
        }
        let Some((parent, _)) = root.rsplit_once('/') else {
            break;
        };
        if root.is_empty() {
            break;
        }
        root = parent.to_string();
    }
    let Some(mut git) = git else {
        return Vec::new();
    };
    if let Ok(data) = fs.read_completion_file(format!("{git}/commondir")).await {
        git = join_remote(&git, String::from_utf8_lossy(&data).trim());
    }
    let mut values = BTreeSet::new();
    if let Ok(data) = fs.read_completion_file(format!("{git}/packed-refs")).await {
        packed(&data, &mut values);
    }
    let mut queue = vec![
        (format!("{git}/refs/heads"), String::new()),
        (format!("{git}/refs/remotes"), String::new()),
    ];
    for _ in 0..32 {
        let Some((directory, prefix)) = queue.pop() else {
            break;
        };
        let Ok(entries) = fs.list_completion_dir(directory.clone()).await else {
            continue;
        };
        for entry in entries {
            let name = format!("{prefix}{}", entry.name);
            if entry.is_dir {
                if name.matches('/').count() < 16 {
                    queue.push((format!("{directory}/{}", entry.name), format!("{name}/")));
                }
            } else if name != "HEAD" && !name.ends_with("/HEAD") && literal_name(&name) {
                values.insert(name);
            }
            if values.len() >= 4096 {
                return values.into_iter().collect();
            }
        }
    }
    values.into_iter().collect()
}
fn join_remote(base: &str, path: &str) -> String {
    if path.starts_with('/') {
        path.into()
    } else {
        format!("{base}/{path}")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn project_metadata_is_data_and_never_executed() {
        assert_eq!(
            scripts(br#"{"scripts":{"build":"touch SHOULD_NOT_RUN","test":"exit 1","bad":12}}"#),
            ["build", "test"]
        );
        let mut branches = BTreeSet::new();
        packed(b"# pack-refs\nabc refs/heads/main\nabc refs/remotes/origin/main\nabc refs/remotes/origin/HEAD\n^def\n", &mut branches);
        assert_eq!(
            branches.into_iter().collect::<Vec<_>>(),
            ["main", "origin/main"]
        );
        let input = "npm run bu";
        let query = Query::parse(&PathQuery::parse(input, input.len(), false).unwrap()).unwrap();
        let candidates = query.candidates(vec!["build".into(), "bad;script".into()]);
        assert_eq!(candidates[0].insert_text, "build");
        assert_eq!(candidates[0].replacement, 8..10);
        assert!(!literal_name("-danger"));
    }
}
