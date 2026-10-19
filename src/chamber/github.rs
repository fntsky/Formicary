use serde::Deserialize;
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Download {
    pub repository: String,
    #[serde(default)]
    pub branch: String,
    #[serde(skip)]
    pub directory: String,
    pub depth: Option<u32>,
}

impl Download {
    fn validate(&self) -> Result<String, String> {
        let repository = repository_url(&self.repository)?;
        if !valid_component(&self.directory) {
            return Err("目标目录只能包含字母、数字、点、短横线和下划线，且不能以点开头。".into());
        }
        if self.depth == Some(0) {
            return Err("下载深度必须大于 0；完整历史请使用 null。".into());
        }
        if !self.branch.is_empty()
            && !Command::new("git")
                .args(["check-ref-format", "--branch", &self.branch])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map_err(|_| "无法运行 Git，请检查服务器是否安装 Git。")?
                .success()
        {
            return Err("分支名称无效。".into());
        }
        Ok(repository)
    }

    pub fn run(&self, root: &Path) -> Result<(), String> {
        let repository = self.validate()?;
        let destination = root.join(&self.directory);
        fs::create_dir(&destination).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                "目标目录已存在，请选择其他目录。".into()
            } else {
                format!("无法创建下载目录：{error}")
            }
        })?;
        let result = self.clone_into(&repository, &destination);
        if result.is_err() {
            let _ = fs::remove_dir_all(&destination);
        }
        result
    }

    fn clone_into(&self, repository: &str, destination: &Path) -> Result<(), String> {
        let mut command = Command::new("git");
        command.args([
            "-c",
            "credential.helper=",
            "-c",
            "core.hooksPath=/dev/null",
            "clone",
            "--single-branch",
        ]);
        if !self.branch.is_empty() {
            command.args(["--branch", &self.branch]);
        }
        if let Some(depth) = self.depth {
            command.args(["--depth", &depth.to_string()]);
        }
        let mut child = command
            .arg("--")
            .arg(repository)
            .arg(destination)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("无法启动 Git：{error}"))?;
        let start = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return if status.success() {
                        Ok(())
                    } else {
                        Err("下载失败，请检查仓库是否公开、分支是否存在以及服务器网络。".into())
                    };
                }
                Ok(None) if start.elapsed() < Duration::from_secs(300) => {
                    thread::sleep(Duration::from_millis(100))
                }
                outcome => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(match outcome {
                        Err(error) => format!("无法读取 Git 状态：{error}"),
                        _ => "下载超时，请尝试更小的深度。".into(),
                    });
                }
            }
        }
    }
}

fn repository_url(repository: &str) -> Result<String, String> {
    let repository = repository.trim();
    let path = repository
        .strip_prefix("https://github.com/")
        .unwrap_or(repository);
    let path = path.trim_end_matches('/').trim_end_matches(".git");
    let parts = path.split('/').collect::<Vec<_>>();
    if parts.len() != 2 || parts.iter().any(|part| !valid_component(part)) {
        return Err("仓库地址必须是 https://github.com/owner/repo 或 owner/repo。".into());
    }
    Ok(format!("https://github.com/{path}.git"))
}

#[derive(serde::Serialize)]
pub struct Branches {
    branches: Vec<String>,
    default_branch: Option<String>,
}

pub fn list_branches(repository: &str) -> Result<Branches, String> {
    use std::io::Read;
    let repository = repository_url(repository)?;
    let mut child = Command::new("git")
        .args([
            "-c",
            "credential.helper=",
            "ls-remote",
            "--symref",
            "--",
            &repository,
            "HEAD",
            "refs/heads/*",
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("无法启动 Git：{error}"))?;
    let stdout = child.stdout.take().unwrap();
    let reader = thread::spawn(move || {
        let mut output = String::new();
        stdout
            .take(8 * 1024 * 1024)
            .read_to_string(&mut output)
            .map(|_| output)
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                let output = reader
                    .join()
                    .map_err(|_| "无法读取分支列表。")?
                    .map_err(|_| "无法读取分支列表。")?;
                return parse_branches(&output);
            }
            Ok(None) if start.elapsed() < Duration::from_secs(30) => {
                thread::sleep(Duration::from_millis(50))
            }
            outcome => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(if matches!(outcome, Ok(None)) {
                    "获取分支超时，请重试。"
                } else {
                    "无法获取分支，请检查公开仓库地址和服务器网络。"
                }
                .into());
            }
        }
    }
}

fn parse_branches(output: &str) -> Result<Branches, String> {
    let mut branches = Vec::new();
    let mut default_branch = None;
    for line in output.lines() {
        if let Some((reference, "HEAD")) = line.split_once('\t') {
            if let Some(name) = reference.strip_prefix("ref: refs/heads/") {
                default_branch = Some(name.to_owned());
            }
        } else if let Some((_, reference)) = line.split_once('\t') {
            if let Some(name) = reference.strip_prefix("refs/heads/") {
                branches.push(name.to_owned());
            }
        }
    }
    branches.sort();
    branches.dedup();
    if branches.is_empty() {
        return Err("仓库中没有可导入的分支。".into());
    }
    Ok(Branches {
        branches,
        default_branch,
    })
}

pub(super) fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && !value.starts_with(['.', '-'])
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn download(repository: &str) -> Download {
        Download {
            repository: repository.into(),
            branch: String::new(),
            directory: "source".into(),
            depth: Some(1),
        }
    }
    /// 验证远端引用能识别默认分支、保留含斜杠的分支名，并拒绝空分支列表。
    /// Verifies that remote references identify the default branch, preserve branch names containing slashes, and reject an empty branch list.
    #[test]
    fn parses_remote_default_and_nested_branch_names() {
        let result = parse_branches("ref: refs/heads/main\tHEAD\nabc\tHEAD\nabc\trefs/heads/main\ndef\trefs/heads/feature/import\n").unwrap();
        assert_eq!(result.default_branch.as_deref(), Some("main"));
        assert_eq!(result.branches, vec!["feature/import", "main"]);
        assert!(parse_branches("").is_err());
    }

    /// 验证支持的 GitHub 仓库地址会规范化为 HTTPS 克隆地址，并拒绝无效地址。
    /// Verifies that supported GitHub repository formats normalize to HTTPS clone URLs and invalid addresses are rejected.
    #[test]
    fn normalizes_github_urls_and_rejects_other_hosts() {
        assert_eq!(
            download("owner/repo").validate().unwrap(),
            "https://github.com/owner/repo.git"
        );
        assert!(
            download("https://github.com/owner/repo.git")
                .validate()
                .is_ok()
        );
        for value in [
            "https://example.com/a/b",
            "https://github.com/a/b/tree/main",
            "../repo",
            "a/b?x",
            "--upload-pack=x",
        ] {
            assert!(download(value).validate().is_err());
        }
    }
    /// 验证已有目标目录会被拒绝，且其中的文件保持不变。
    /// Verifies that an existing destination is rejected and its files remain unchanged.
    #[test]
    fn preserves_existing_destination() {
        let root =
            std::env::temp_dir().join(format!("formicary-download-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("source")).unwrap();
        fs::write(root.join("source/keep.txt"), "keep").unwrap();
        assert!(
            download("owner/repo")
                .run(&root)
                .unwrap_err()
                .contains("已存在")
        );
        assert_eq!(
            fs::read_to_string(root.join("source/keep.txt")).unwrap(),
            "keep"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// 验证拒绝不安全的目录名和零下载深度，同时允许下载完整历史。
    /// Verifies that unsafe directory names and zero depth are rejected while full-history downloads are allowed.
    #[test]
    fn rejects_unsafe_destinations_and_invalid_depth() {
        for directory in ["../outside", "/tmp/source", ".", "-source", "nested/path"] {
            let mut request = download("a/b");
            request.directory = directory.into();
            assert!(request.validate().is_err());
        }
        let mut request = download("a/b");
        request.depth = Some(0);
        assert!(request.validate().is_err());
        request.depth = None;
        assert!(request.validate().is_ok());
    }
}
