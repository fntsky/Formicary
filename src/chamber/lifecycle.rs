use super::*;
use crate::app::App;

impl ChamberManager {
    pub fn stopped(&self, name: &str) -> bool {
        self.root.join(name).join(".stopped").exists()
    }

    pub fn stop(&mut self, name: &str, app: &App) -> Result<(), String> {
        let c = self
            .chambers
            .iter()
            .find(|c| c.name == name)
            .ok_or("Chamber 不存在")?;
        fs::write(self.root.join(name).join(".stopped"), b"").map_err(|e| e.to_string())?;
        for job in c.jobs() {
            let _ = c.pause_job(job.id());
            job.terminate_shell();
            c.job_service().remove_job(job.id());
        }
        c.close(app).map_err(|e| e.to_string())
    }

    fn detach(&mut self, name: &str, app: &App) -> Result<(), String> {
        self.stop(name, app)?;
        let index = self.chambers.iter().position(|c| c.name == name).unwrap();
        self.chambers.remove(index);
        Ok(())
    }

    fn activate(&mut self, name: &str, app: &App) -> Result<(), String> {
        let mut loaded =
            Self::load_mode(&self.root, Some(name), false).map_err(|e| e.to_string())?;
        let c = loaded.chambers.pop().ok_or("没有找到 Chamber 配置")?;
        self.add_chamber(c, app).map_err(|e| e.to_string())?;
        let c = self.chambers.iter().find(|c| c.name == name).unwrap();
        let result = (|| {
            let client = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(2))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|e| e.to_string())?;
            for mount in c.proxy_http_mounts() {
                let path = mount.health_path.as_deref().unwrap_or("/healthz");
                if !path.starts_with('/') || path.starts_with("//") {
                    return Err("health_path 必须是绝对 URL 路径".into());
                }
                let url = format!("{}{}", mount.upstream.trim_end_matches('/'), path);
                let deadline = std::time::Instant::now() + Duration::from_secs(20);
                loop {
                    if let Some(proxy) = app
                        .proxy_http_mount(&mount.route)
                        .map_err(|e| e.to_string())?
                    {
                        if proxy.shell().is_finished() {
                            return Err("代理进程提前退出".into());
                        }
                    }
                    if client
                        .get(&url)
                        .send()
                        .is_ok_and(|r| r.status().is_success())
                    {
                        break;
                    }
                    if std::time::Instant::now() >= deadline {
                        return Err(format!("健康检查失败：{url}"));
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
            let source =
                fs::read_to_string(c.directory.join("chamber.toml")).map_err(|e| e.to_string())?;
            let config = parse_chamber_file(&source).map_err(|e| e.to_string())?;
            fs::remove_file(self.root.join(name).join(".stopped"))
                .or_else(|e| {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        Ok(())
                    } else {
                        Err(e)
                    }
                })
                .map_err(|e| e.to_string())?;
            for (job, config) in c.jobs().iter().zip(config.jobs) {
                if config.enabled {
                    c.start_job(job.id()).map_err(|e| e.to_string())?;
                }
            }
            Ok(())
        })();
        if result.is_err() {
            self.detach(name, app)?;
        }
        result
    }

    pub fn start(&mut self, name: &str, app: &App) -> Result<(), String> {
        if !self.stopped(name) {
            return Ok(());
        }
        self.detach(name, app)?;
        match self.activate(name, app) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.restore_stopped(name)?;
                Err(e)
            }
        }
    }

    fn restore_stopped(&mut self, name: &str) -> Result<(), String> {
        fs::write(self.root.join(name).join(".stopped"), b"").map_err(|e| e.to_string())?;
        let mut loaded =
            Self::load_mode(&self.root, Some(name), false).map_err(|e| e.to_string())?;
        if let Some(c) = loaded.chambers.pop() {
            self.chambers.push(c);
        }
        Ok(())
    }

    pub fn delete(&mut self, name: &str, app: &App) -> Result<(), String> {
        self.detach(name, app)?;
        fs::remove_dir_all(self.root.join(name)).map_err(|e| e.to_string())
    }

    pub fn releases(&self, name: &str) -> Vec<String> {
        let mut versions = fs::read_dir(self.root.join(name).join("releases"))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| e.file_name().into_string().ok())
            .collect::<Vec<_>>();
        versions.sort();
        versions
    }

    pub fn delete_release(&self, name: &str, commit: &str) -> Result<(), String> {
        if !crate::chamber::github::valid_component(commit) {
            return Err("版本无效".into());
        }
        let base = self.root.join(name);
        let release = base.join("releases").join(commit);
        if fs::canonicalize(base.join("current")).ok() == fs::canonicalize(&release).ok() {
            return Err("不能删除当前版本".into());
        }
        fs::remove_dir_all(release).map_err(|e| e.to_string())
    }

    fn point(&self, name: &str, release: &Path) -> Result<(), String> {
        let base = self.root.join(name);
        let temporary = base.join(format!("current-{}", uuid::Uuid::new_v4()));
        std::os::unix::fs::symlink(release, &temporary).map_err(|e| e.to_string())?;
        fs::rename(&temporary, base.join("current")).map_err(|e| e.to_string())
    }

    pub fn install(
        &mut self,
        download: &crate::chamber::github::Download,
        expected: Option<&str>,
        app: &App,
    ) -> Result<String, String> {
        let staging = format!("import-{}", uuid::Uuid::new_v4());
        let request = crate::chamber::github::Download {
            repository: download.repository.clone(),
            branch: download.branch.clone(),
            depth: download.depth,
            directory: staging.clone(),
        };
        request.run(&self.root)?;
        let staged = self.root.join(&staging);
        let result = self.install_staged(&staged, expected, app);
        let _ = fs::remove_dir_all(staged);
        result
    }

    fn install_staged(
        &mut self,
        staged: &Path,
        expected: Option<&str>,
        app: &App,
    ) -> Result<String, String> {
        let name = imported_chamber_name(staged)?;
        if expected.is_some_and(|n| n != name) {
            return Err("升级版本的 Chamber 名称不一致".into());
        }
        let base = self.root.join(&name);
        if expected.is_none() && base.exists() {
            return Err("Chamber 已存在，请使用升级".into());
        }
        if expected.is_some() && !base.join("current").exists() {
            return Err("旧布局请先将持久数据迁移到 data，并将程序迁移到 releases/<commit>，创建 current 链接后再升级".into());
        }
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(staged)
            .args(["rev-parse", "HEAD"])
            .output()
            .map_err(|e| e.to_string())?;
        let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if commit.len() != 40 || !commit.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err("无法读取确切 commit".into());
        }
        let release =
            std::path::absolute(base.join("releases").join(&commit)).map_err(|e| e.to_string())?;
        if release.exists() {
            return Err("该 commit 已存在".into());
        }
        fs::create_dir_all(base.join("data")).map_err(|e| e.to_string())?;
        fs::create_dir_all(base.join("releases")).map_err(|e| e.to_string())?;
        fs::rename(staged, &release).map_err(|e| e.to_string())?;
        let config = parse_chamber_file(
            &fs::read_to_string(release.join("chamber.toml")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if let Some(prepare) = config.chamber.prepare {
            let output = crate::service::shell_runner::ShellRunner::run_with_timeout(
                &release,
                &prepare,
                Duration::from_secs(300),
            )
            .map_err(|e| e.to_string());
            if !output.is_ok_and(|o| o.status.success()) {
                let _ = fs::remove_dir_all(&release);
                if expected.is_none() {
                    let _ = fs::remove_dir_all(&base);
                }
                return Err("新版本准备或构建失败，旧版本保持运行".into());
            }
        }
        // Validate using an isolated root before touching the active symlink or runtime.
        let check_root = self.root.join(format!("check-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&check_root).map_err(|e| e.to_string())?;
        let check_base = check_root.join(&name);
        fs::create_dir(&check_base).map_err(|e| e.to_string())?;
        std::os::unix::fs::symlink(&release, check_base.join("current"))
            .map_err(|e| e.to_string())?;
        let checked = Self::load_mode(&check_root, Some(&name), false).map_err(|e| e.to_string());
        let validation = checked.and_then(|loaded| {
            let candidate = &loaded.chambers[0];
            let routes = candidate
                .static_web_mounts()
                .iter()
                .map(|m| &m.route)
                .chain(candidate.proxy_http_mounts().iter().map(|m| &m.route));
            for route in routes {
                if self.chambers.iter().filter(|c| c.name != name).any(|c| {
                    c.static_web_mounts().iter().any(|m| &m.route == route)
                        || c.proxy_http_mounts().iter().any(|m| &m.route == route)
                }) {
                    return Err(format!("路由冲突：{route}"));
                }
            }
            Ok(())
        });
        let _ = fs::remove_dir_all(check_root);
        if let Err(e) = validation {
            let _ = fs::remove_dir_all(&release);
            if expected.is_none() {
                let _ = fs::remove_dir_all(&base);
            }
            return Err(e);
        }
        let old = fs::canonicalize(base.join("current")).ok();
        let was_stopped = self.stopped(&name);
        if expected.is_some() {
            self.detach(&name, app)?;
        }
        let switched = self
            .point(&name, &release)
            .and_then(|_| self.activate(&name, app));
        if let Err(e) = switched {
            if let Some(old) = old {
                self.point(&name, &old)?;
                if was_stopped {
                    self.restore_stopped(&name)?;
                } else if let Err(rollback) = self.activate(&name, app) {
                    self.restore_stopped(&name)?;
                    return Err(format!("{e}；回滚启动失败：{rollback}"));
                }
            } else {
                let _ = fs::remove_dir_all(&base);
            }
            return Err(format!("{e}；已回滚"));
        }
        if was_stopped {
            self.stop(&name, app)?;
        }
        Ok(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn staged(root: &Path, text: &str) -> PathBuf {
        let path = root.join(format!("stage-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("chamber.toml"), text).unwrap();
        for args in [
            vec!["init"],
            vec!["add", "."],
            vec![
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "test",
            ],
        ] {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(&path)
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
        path
    }
    /// 验证升级、健康检查失败回滚、持久数据保留、关停重载和删除保护。
    #[test]
    fn upgrade_rollback_stop_and_delete() {
        let root =
            std::env::temp_dir().join(format!("formicary-lifecycle-{}", uuid::Uuid::new_v4()));
        let mut manager = ChamberManager::load(&root).unwrap();
        let app = App::new();
        let config = "[chamber]\nname='test'\nschema_version=1\n";
        let first = staged(&root, config);
        manager.install_staged(&first, None, &app).unwrap();
        let original = fs::canonicalize(root.join("test/current")).unwrap();
        fs::write(root.join("test/data/keep"), "persistent").unwrap();
        let broken = staged(
            &root,
            &format!(
                "{config}[proxyhttp]\nscript='exit 1'\nroute='/broken'\nupstream='http://127.0.0.1:1'\n"
            ),
        );
        assert!(manager.install_staged(&broken, Some("test"), &app).is_err());
        assert_eq!(
            fs::canonicalize(root.join("test/current")).unwrap(),
            original
        );
        assert!(!manager.stopped("test"));
        let second = staged(&root, &format!("{config}# new release\n"));
        manager.install_staged(&second, Some("test"), &app).unwrap();
        let current = manager.chambers()[0]
            .directory
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert!(manager.delete_release("test", &current).is_err());
        manager
            .delete_release("test", original.file_name().unwrap().to_str().unwrap())
            .unwrap();
        assert_eq!(
            fs::read_to_string(root.join("test/data/keep")).unwrap(),
            "persistent"
        );
        manager.stop("test", &app).unwrap();
        drop(manager);
        let mut manager = ChamberManager::load(&root).unwrap();
        assert!(manager.stopped("test"));
        manager.start("test", &app).unwrap();
        manager.delete("test", &app).unwrap();
        assert!(!root.join("test").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
