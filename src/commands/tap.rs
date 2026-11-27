use crate::config::{self, Config, TapRole};
use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::Command;

/// 克隆/下载 tap 到目标目录。优先 git clone，GitHub 仓库失败时回退 codeload tarball。
/// 返回 `Ok(true)` 表示成功，`Ok(false)` 表示失败但已清理。
fn fetch_tap(repo_url: &str, dir: &PathBuf, depth: Option<&str>) -> Result<bool> {
    let git_args = if let Some(d) = depth {
        vec!["clone", "--depth", d, repo_url, dir.to_str().unwrap()]
    } else {
        vec!["clone", repo_url, dir.to_str().unwrap()]
    };

    let git_ok = Command::new("git").args(&git_args).status().map(|s| s.success()).unwrap_or(false);

    if git_ok {
        return Ok(true);
    }

    // 失败了，清理残留
    let _ = std::fs::remove_dir_all(dir);

    // 如果是 GitHub 仓库，尝试 codeload tarball 回退
    if let Some((org, repo)) = parse_github_org_repo(repo_url) {
        let tarball_url = format!("https://codeload.github.com/{}/{}/tar.gz/refs/heads/main", org, repo);
        eprintln!("  git clone 失败，尝试 codeload tarball: {}", tarball_url);
        if let Ok(true) = download_tarball(&tarball_url, dir) {
            return Ok(true);
        }
    }

    Ok(false)
}

fn parse_github_org_repo(url: &str) -> Option<(&str, &str)> {
    let u = url.trim_end_matches(".git");
    // https://github.com/org/repo 或 git@github.com:org/repo
    u.split_once("github.com/")
        .or_else(|| u.split_once("github.com:"))
        .and_then(|(_, rest)| {
            let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
            if parts.len() >= 2 { Some((parts[0], parts[1])) } else { None }
        })
}

/// 从 URL 下载 tarball 并解压到 dir（会自动剥离顶层目录）
fn download_tarball(url: &str, dir: &PathBuf) -> Result<bool> {
    let tmp = std::env::temp_dir().join(format!("fb-tap-{}.tar.gz", std::process::id()));
    let curl_ok = Command::new("curl")
        .args(["-sSL", "--max-time", "120", "-o", tmp.to_str().unwrap(), url])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !curl_ok {
        return Ok(false);
    }

    // 创建目标父目录
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // 解压到临时目录，然后搬运内容
    let tmp_extract = std::env::temp_dir().join(format!("fb-tap-extract-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_extract);
    std::fs::create_dir_all(&tmp_extract)?;
    let tar_ok = Command::new("tar")
        .args(["xzf", tmp.to_str().unwrap(), "-C", tmp_extract.to_str().unwrap()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let _ = std::fs::remove_file(&tmp);
    if !tar_ok {
        let _ = std::fs::remove_dir_all(&tmp_extract);
        return Ok(false);
    }

    // 找 tarball 解出来的顶层目录（通常是 org-repo-main 或 org-repo-master）
    let top = std::fs::read_dir(&tmp_extract)?
        .flatten()
        .find(|e| e.path().is_dir());
    let Some(top_dir) = top else {
        let _ = std::fs::remove_dir_all(&tmp_extract);
        return Ok(false);
    };

    // 把顶层目录内容移到目标 dir
    std::fs::rename(top_dir.path(), dir).or_else(|_| {
        // 跨设备时 rename 会失败，改用 cp
        let src = format!("{}/.", top_dir.path().display());
        let ok = Command::new("cp")
            .args(["-R", &src, dir.to_str().unwrap()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok { Ok(()) } else { Err(std::io::Error::new(std::io::ErrorKind::Other, "cp failed")) }
    })?;
    let _ = std::fs::remove_dir_all(&tmp_extract);
    Ok(true)
}

/// `fb tap` 入口：
/// - 无参数：列出已添加的 taps（带角色标签）
/// - `--core`：一键添加 seeds-core + fields-core 两个官方核心 tap
/// - `--remove`：移除指定 tap
/// - 其他：克隆 `user/repo`（默认映射到 GitHub 的 `user/homebrew-repo`）
pub fn run(
    cfg: &Config,
    name: Option<&str>,
    url: Option<&str>,
    remove: bool,
    core: bool,
) -> Result<()> {
    if core {
        return add_core(cfg, name.is_none());
    }
    match name {
        None => list(cfg),
        Some(n) if remove => untap(cfg, n),
        Some(n) => add(cfg, n, url),
    }
}

fn add_core(cfg: &Config, both: bool) -> Result<()> {
    let tap = if both {
        // 同时添加两个
        let _ = add_builtin(cfg, &config::SEEDS_CORE);
        add_builtin(cfg, &config::FIELDS_CORE)
    } else {
        // 只添加指定名
        let _ = add_builtin(cfg, &config::SEEDS_CORE);
        Ok(())
    };
    tap
}

fn add_builtin(cfg: &Config, builtin: &config::BuiltinTap) -> Result<()> {
    if cfg.has_builtin_tap(builtin) {
        println!("==> tap 已存在: {} [{}]", builtin.name, builtin.role.label());
        return Ok(());
    }
    // 内置核心 tap 用配置的 GitHub org
    let remote = format!(
        "https://github.com/{}/{}.git",
        builtin.github_org,
        builtin.repo
    );
    let dir = cfg.core_taps_root().join(builtin.repo);
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }

    println!("==> 克隆 {} [{}]", builtin.name, builtin.role.label());
    println!("    描述: {}", builtin.description);
    let ok = fetch_tap(&remote, &dir, Some("1"))?;
    if !ok {
        eprintln!(
            "警告: 克隆 {} 失败，跳过（网络可能不可达）",
            builtin.name
        );
        return Ok(());
    }

    // 写入 .fieldsbrew.toml 标记 tap 角色（不覆盖仓库自带的）
    let meta = dir.join(".fieldsbrew.toml");
    if !meta.exists() {
        let role_str = match builtin.role {
            TapRole::SeedsCore => "seeds",
            TapRole::FieldsCore => "fields",
            TapRole::User => "user",
        };
        std::fs::write(
            &meta,
            format!(
                "# Fieldsbrew tap 元信息\n[tap]\nrole = \"{role_str}\"\nname = \"{}\"\n",
                builtin.name
            ),
        )?;
    }

    let count = count_formulas(&dir);
    println!(
        "==> 已添加 tap: {} [{}]（{count} 个 formula）",
        builtin.name,
        builtin.role.label()
    );
    if count == 0 {
        println!("    提示: tap 中未找到 Formula/*.toml 或根目录 *.toml");
    }
    Ok(())
}

fn add(cfg: &Config, name: &str, url: Option<&str>) -> Result<()> {
    let (user, repo, remote) = match url {
        Some(u) => {
            let (user, repo) = parse_tap_name(name)?;
            let remote = if u.contains("://") || u.starts_with("git@") {
                u.to_string()
            } else {
                format!("https://github.com/{user}/{repo}.git")
            };
            (user, repo, remote)
        }
        None if name.contains("://") || name.starts_with("git@") => {
            let (user, repo) = parse_remote(name)?;
            (user, repo, name.to_string())
        }
        None => {
            let (user, repo) = parse_tap_name(name)?;
            let remote = if repo.starts_with("homebrew-") {
                format!("https://github.com/{user}/{repo}.git")
            } else {
                format!("https://github.com/{user}/homebrew-{repo}.git")
            };
            (user, repo, remote)
        }
    };

    let dir = cfg.taps_root().join(&user).join(&repo);
    if dir.exists() {
        println!("==> tap 已存在: {}/{}", user, display_repo(&repo));
        return Ok(());
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }

    println!("==> 克隆 {remote}");
    let ok = fetch_tap(&remote, &dir, None)?;
    if !ok {
        bail!("克隆 tap 失败: {remote}");
    }

    let count = count_formulas(&dir);
    let role = cfg.tap_info(&dir).role;
    println!(
        "==> 已添加 tap: {}/{} [{}]（{count} 个 formula）",
        user,
        display_repo(&repo),
        role.label()
    );
    if count == 0 {
        println!("    提示: tap 中未找到 Formula/*.toml、HomebrewFormula/*.toml 或根目录 *.toml");
    }
    Ok(())
}

fn untap(cfg: &Config, name: &str) -> Result<()> {
    let (user, repo) = parse_tap_name(name)?;
    let dir = cfg.taps_root().join(&user).join(&repo);
    if !dir.exists() {
        bail!("未添加该 tap: {}/{}", user, display_repo(&repo));
    }
    let role = cfg.tap_info(&dir).role;
    std::fs::remove_dir_all(&dir)
        .with_context(|| format!("移除 tap 失败: {}", dir.display()))?;
    let user_dir = cfg.taps_root().join(&user);
    if user_dir.is_dir() && std::fs::read_dir(&user_dir)?.next().is_none() {
        let _ = std::fs::remove_dir(&user_dir);
    }
    println!(
        "==> 已移除 tap: {}/{} [{}]",
        user,
        display_repo(&repo),
        role.label()
    );
    Ok(())
}

fn list(cfg: &Config) -> Result<()> {
    let infos = cfg.taps_sorted();
    if infos.is_empty() {
        println!("没有已添加的 tap。使用 `fb tap --core` 添加官方核心 tap，或 `fb tap user/repo` 添加自定义。");
        return Ok(());
    }
    println!("{:<30} {:<14} {}", "TAP", "类型", "FORMULA");
    println!("{}", "-".repeat(54));
    for info in &infos {
        let count = count_formulas(&info.path);
        let label = info.role.label();
        println!("{:<30} {:<14} {}", info.short_name(), label, count);
    }
    Ok(())
}

/// 解析 `user/repo`；repo 部分若省略 `homebrew-` 前缀则补全，用于目录名。
fn parse_tap_name(name: &str) -> Result<(String, String)> {
    let parts: Vec<&str> = name.split('/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        bail!("tap 名称应为 user/repo 形式，例如: fb tap user/homebrew-tools");
    }
    let user = validate_part(parts[0])?;
    let repo = validate_part(parts[1])?;
    let repo = if repo.starts_with("homebrew-") {
        repo
    } else {
        format!("homebrew-{repo}")
    };
    Ok((user, repo))
}

/// 从 git URL 提取 user/repo（用于 `fb tap <url>`）
fn parse_remote(url: &str) -> Result<(String, String)> {
    let cleaned = url.trim_end_matches('/').trim_end_matches(".git");
    let path = if let Some((_, rest)) = cleaned.split_once("://") {
        rest.split('/').skip(1).collect::<Vec<_>>().join("/")
    } else if let Some((_, rest)) = cleaned.split_once(':') {
        rest.to_string()
    } else {
        cleaned.to_string()
    };
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() < 2 {
        bail!("无法从 URL 推导 tap 名称: {url}（请改用 user/repo 形式）");
    }
    let user = validate_part(parts[parts.len() - 2])?;
    let repo = validate_part(parts[parts.len() - 1])?;
    let repo = if repo.starts_with("homebrew-") {
        repo
    } else {
        format!("homebrew-{repo}")
    };
    Ok((user, repo))
}

fn validate_part(s: &str) -> Result<String> {
    if s.is_empty()
        || !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        bail!("非法的 tap 名称片段 `{s}`（仅允许字母、数字、- _ .）");
    }
    Ok(s.to_string())
}

fn display_repo(repo: &str) -> &str {
    repo.strip_prefix("homebrew-").unwrap_or(repo)
}

fn count_formulas(dir: &std::path::Path) -> usize {
    let mut count = 0;
    for sub in ["Formula", "HomebrewFormula", ""] {
        let d = if sub.is_empty() { dir.to_path_buf() } else { dir.join(sub) };
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        count += entries
            .flatten()
            .filter(|e| {
                let p = e.path();
                p.extension().map(|x| x == "toml").unwrap_or(false)
                    && p.file_name().and_then(|n| n.to_str()) != Some(".fieldsbrew.toml")
            })
            .count();
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::fs;

    #[test]
    fn test_parse_tap_name() {
        assert_eq!(
            parse_tap_name("user/tools").unwrap(),
            ("user".to_string(), "homebrew-tools".to_string())
        );
        assert_eq!(
            parse_tap_name("user/homebrew-tools").unwrap(),
            ("user".to_string(), "homebrew-tools".to_string())
        );
        assert!(parse_tap_name("user").is_err());
        assert!(parse_tap_name("user/tools/extra").is_err());
        assert!(parse_tap_name("user/tool$").is_err());
    }

    #[test]
    fn test_parse_remote() {
        assert_eq!(
            parse_remote("https://github.com/user/homebrew-tools.git").unwrap(),
            ("user".to_string(), "homebrew-tools".to_string())
        );
        assert_eq!(
            parse_remote("git@github.com:user/tools.git").unwrap(),
            ("user".to_string(), "homebrew-tools".to_string())
        );
        assert!(parse_remote("https://github.com/user").is_err());
    }

    #[test]
    fn test_tap_role_detection() {
        let tmp = std::env::temp_dir().join(format!("fb-tap-test-{}", std::process::id()));
        let cfg = Config {
            prefix: tmp.clone(),
            verbose: false,
        };
        let _ = fs::remove_dir_all(&tmp);
        let roots = cfg.taps_root().join("myuser").join("homebrew-mytap");
        fs::create_dir_all(&roots).unwrap();
        // 无 meta 文件时默认 User
        assert_eq!(cfg.tap_info(&roots).role, TapRole::User);

        // 写入 meta 标记为 fields
        fs::write(roots.join(".fieldsbrew.toml"), "[tap]\nrole = \"fields\"\n").unwrap();
        assert_eq!(cfg.tap_info(&roots).role, TapRole::FieldsCore);

        // seeds
        fs::write(roots.join(".fieldsbrew.toml"), "[tap]\nrole = \"seeds\"\n").unwrap();
        assert_eq!(cfg.tap_info(&roots).role, TapRole::SeedsCore);

        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn test_tap_priority_ordering() {
        let tmp = std::env::temp_dir().join(format!("fb-tap-prio-{}", std::process::id()));
        let cfg = Config {
            prefix: tmp.clone(),
            verbose: false,
        };
        let _ = fs::remove_dir_all(&tmp);

        // 构造三个 tap：fields-core、seeds-core、普通
        for (user, repo, meta) in [
            ("fieldsbrew", "fields-core", Some("[tap]\nrole = \"fields\"\n")),
            ("fieldsbrew", "seeds-core", Some("[tap]\nrole = \"seeds\"\n")),
            ("someone", "homebrew-tools", None),
        ] {
            let d = cfg.taps_root().join(user).join(repo).join("Formula");
            fs::create_dir_all(&d).unwrap();
            if let Some(m) = meta {
                fs::write(cfg.taps_root().join(user).join(repo).join(".fieldsbrew.toml"), m).unwrap();
            }
        }

        let infos = cfg.taps_sorted();
        assert_eq!(infos[0].role, TapRole::FieldsCore);
        assert_eq!(infos[1].role, TapRole::SeedsCore);
        assert_eq!(infos[2].role, TapRole::User);

        fs::remove_dir_all(&tmp).ok();
    }
}
