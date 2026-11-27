use crate::version;
use anyhow::Result;
use serde::Deserialize;
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct BrewInstall {
    pub name: String,
    pub versions: Vec<String>,
    pub current: String,
}

impl From<BrewInstall> for String {
    fn from(b: BrewInstall) -> String {
        format!("{} ({})", b.name, b.current)
    }
}
impl From<&BrewInstall> for String {
    fn from(b: &BrewInstall) -> String {
        format!("{} ({})", b.name, b.current)
    }
}

/// 探测 Homebrew 前缀（只读，不修改任何 Homebrew 文件）。
/// 优先 $HOMEBREW_PREFIX，其次 Intel 默认 /usr/local、Apple Silicon 默认 /opt/homebrew。
pub fn prefix() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("HOMEBREW_PREFIX") {
        let p = p.trim();
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    for p in ["/usr/local", "/opt/homebrew"] {
        let path = Path::new(p);
        if path.join("Cellar").is_dir() {
            return Some(path.to_path_buf());
        }
    }
    None
}

pub fn cellar() -> Option<PathBuf> {
    prefix()
        .map(|p| p.join("Cellar"))
        .filter(|p| p.is_dir())
}

/// 扫描 Homebrew Cellar。只读取目录结构，不调用 brew，也不修改文件。
pub fn installed() -> Vec<BrewInstall> {
    if let Some(cellar) = cellar() {
        if let Ok(list) = scan_cellar(&cellar) {
            if !list.is_empty() {
                return list;
            }
        }
    }
    brew_list_json().unwrap_or_default()
}

pub fn scan_cellar(cellar: &Path) -> Result<Vec<BrewInstall>> {
    let mut out = Vec::new();
    if !cellar.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(cellar)?.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let mut versions = Vec::new();
        for v in std::fs::read_dir(entry.path())?.flatten() {
            if !v.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let ver = v.file_name().to_string_lossy().to_string();
            if !ver.starts_with('.') {
                versions.push(ver);
            }
        }
        if versions.is_empty() {
            continue;
        }
        let current = versions
            .iter()
            .max_by(|a, b| version::compare(a, b))
            .cloned()
            .unwrap_or_default();
        out.push(BrewInstall {
            name,
            versions,
            current,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

pub fn brew_binary() -> Option<PathBuf> {
    if let Some(p) = prefix() {
        let b = p.join("bin/brew");
        if b.exists() {
            return Some(b);
        }
    }
    let out = Command::new("which").arg("brew").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(PathBuf::from(s))
    }
}

#[derive(Deserialize)]
struct BrewList {
    #[serde(default)]
    formulae: Vec<BrewFormula>,
}

#[derive(Deserialize)]
struct BrewFormula {
    name: String,
    #[serde(default)]
    versions: Vec<String>,
}

/// Cellar 不可用时的兜底：`brew list --json=v2 --formula`
fn brew_list_json() -> Option<Vec<BrewInstall>> {
    let brew = brew_binary()?;
    let out = Command::new(brew)
        .args(["list", "--json=v2", "--formula"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let parsed: BrewList = serde_json::from_slice(&out.stdout).ok()?;
    let mut list: Vec<BrewInstall> = parsed
        .formulae
        .into_iter()
        .filter(|f| !f.versions.is_empty())
        .map(|f| BrewInstall {
            current: f.versions.first().cloned().unwrap_or_default(),
            versions: f.versions,
            name: f.name,
        })
        .collect();
    list.sort_by(|a, b| a.name.cmp(&b.name));
    Some(list)
}

/// 找出 Fieldsbrew 与 Homebrew 都安装了的同名包
pub fn overlap(fb_names: &[String]) -> Vec<BrewInstall> {
    let brew = installed();
    brew.into_iter()
        .filter(|b| fb_names.iter().any(|n| n == &b.name))
        .collect()
}

/// PATH 中 prefix/bin 与 Homebrew bin 的先后顺序（返回 (fb 位置, brew 位置)）
pub fn path_positions(path: &str, fb_prefix: &Path, brew_prefix: &Path) -> (Option<usize>, Option<usize>) {
    let fb_bin = fb_prefix.join("bin").display().to_string();
    let brew_bin = brew_prefix.join("bin").display().to_string();
    let parts: Vec<&str> = path.split(':').collect();
    let fb = parts.iter().position(|p| *p == fb_bin);
    let brew = parts.iter().position(|p| *p == brew_bin);
    (fb, brew)
}

/// 比较 Homebrew 与 Fieldsbrew 的版本；供 migrate 输出提示。
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    version::compare(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_cellar() {
        let base = std::env::temp_dir().join(format!("fb-brew-{}", std::process::id()));
        let cellar = base.join("Cellar");
        std::fs::create_dir_all(cellar.join("foo/1.2.3/bin")).unwrap();
        std::fs::create_dir_all(cellar.join("bar/2.0.0")).unwrap();
        std::fs::create_dir_all(cellar.join("bar/2.1.0")).unwrap();
        std::fs::create_dir_all(cellar.join("foo/.metadata")).unwrap();
        std::fs::create_dir_all(cellar.join("empty")).unwrap();

        let list = scan_cellar(&cellar).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "bar");
        assert_eq!(list[0].current, "2.1.0");
        assert_eq!(list[1].name, "foo");
        assert_eq!(list[1].current, "1.2.3");
        assert_eq!(list[1].versions.len(), 1);
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn test_path_positions() {
        let fb = Path::new("/opt/fieldsbrew");
        let brew = Path::new("/usr/local");
        let (f, b) = path_positions(
            "/usr/local/bin:/usr/bin:/opt/fieldsbrew/bin",
            fb,
            brew,
        );
        assert_eq!(f, Some(2));
        assert_eq!(b, Some(0));
        let (f, b) = path_positions("/usr/bin:/usr/local/bin", fb, brew);
        assert_eq!(f, None);
        assert_eq!(b, Some(1));
    }
}
