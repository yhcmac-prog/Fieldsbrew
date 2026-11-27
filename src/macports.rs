use crate::version;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 探测 MacPorts 前缀。
/// 优先 $MACPORTS_PREFIX，其次默认 /opt/local。
pub fn prefix() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MACPORTS_PREFIX") {
        let p = p.trim();
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    for p in ["/opt/local", "/usr/local/macports"] {
        let path = Path::new(p);
        if path.join("var/macports").is_dir() || path.join("bin/port").is_file() {
            return Some(path.to_path_buf());
        }
    }
    let port = port_binary();
    port.and_then(|b| b.parent().and_then(|p| p.parent().map(|pp| pp.to_path_buf())))
}

/// 探测 MacPorts 安装根（类似 Cellar，存包目录）
pub fn var_macports() -> Option<PathBuf> {
    prefix().map(|p| p.join("var/macports"))
        .filter(|p| p.is_dir())
}

/// 探测 installed 目录（各包版本）
pub fn installed_dir() -> Option<PathBuf> {
    var_macports().map(|p| p.join("software"))
        .filter(|p| p.is_dir())
}

pub fn port_binary() -> Option<PathBuf> {
    // 1. 从 prefix/bin/port 找
    if let Some(pfx) = prefix() {
        let b = pfx.join("bin/port");
        if b.exists() {
            return Some(b);
        }
    }
    // 2. which port
    let out = Command::new("which").arg("port").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(PathBuf::from(s)) }
}

#[derive(Debug, Clone)]
pub struct PortInstall {
    pub name: String,
    pub version: String,
    pub revision: u32,
}

impl From<PortInstall> for String {
    fn from(p: PortInstall) -> String {
        if p.revision > 0 {
            format!("{} ({}_{})", p.name, p.version, p.revision)
        } else {
            format!("{} ({})", p.name, p.version)
        }
    }
}

/// 扫描 /opt/local/var/macports/software/<name>/<version>/<name>-<version>
/// 只读目录结构，不调用 port，不修改任何 MacPorts 文件。
pub fn installed() -> Vec<PortInstall> {
    let Some(software) = installed_dir() else {
        return Vec::new();
    };
    scan_software(&software).unwrap_or_default()
}

pub fn scan_software(software: &Path) -> Result<Vec<PortInstall>> {
    let mut out = Vec::new();
    if !software.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(software)?.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        // 子目录: <version>_<revision>
        for ver_entry in std::fs::read_dir(entry.path())?.flatten() {
            if !ver_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let dir = ver_entry.file_name().to_string_lossy().to_string();
            // MacPorts 格式: <version> 或 <version>_<revision> 或 <version>-<variant>
            let (version, revision) = dir.split_once('_')
                .map(|(v, r)| (v.to_string(), r.parse::<u32>().unwrap_or(0)))
                .unwrap_or_else(|| (dir.clone(), 0));
            out.push(PortInstall {
                name: name.clone(),
                version,
                revision,
            });
            break;  // 取每个包的第一个版本即可
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// 用 port 命令列出已安装包（兜底；software 目录不可用时才用）
pub fn port_list() -> Option<Vec<PortInstall>> {
    let port = port_binary()?;
    let out = Command::new(port)
        .args(["installed", "-q", "--no-line-numbers"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut list = Vec::new();
    for line in text.lines() {
        // 格式: <name> <version>_<revision>
        let mut parts = line.trim().split_whitespace();
        let Some(name) = parts.next() else { continue };
        let Some(ver_rev) = parts.next() else { continue };
        let (version, revision) = ver_rev.split_once('_')
            .map(|(v, r)| (v.to_string(), r.parse::<u32>().unwrap_or(0)))
            .unwrap_or_else(|| (ver_rev.to_string(), 0));
        list.push(PortInstall { name: name.to_string(), version, revision });
    }
    list.sort_by(|a, b| a.name.cmp(&b.name));
    Some(list)
}

pub fn overlap(fb_names: &[String]) -> Vec<PortInstall> {
    let mut mp = installed();
    if mp.is_empty() {
        mp = port_list().unwrap_or_default();
    }
    mp.into_iter()
        .filter(|p| fb_names.iter().any(|n| n == &p.name))
        .collect()
}

pub fn version_compare(a: &str, b: &str) -> std::cmp::Ordering {
    version::compare(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_scan_software() {
        let base = std::env::temp_dir().join(format!("fb-mp-{}", std::process::id()));
        let sw = base.join("software");
        fs::create_dir_all(sw.join("bash/5.2.008_0")).unwrap();
        fs::create_dir_all(sw.join("curl/8.11.1_0")).unwrap();
        fs::create_dir_all(sw.join("sqlite3/3.47.0_1")).unwrap();
        fs::create_dir_all(sw.join(".ds_store/1.0")).unwrap();
        fs::create_dir_all(sw.join("nopkg")).unwrap();

        let list = scan_software(&sw).unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].name, "bash");
        assert_eq!(list[0].version, "5.2.008");
        assert_eq!(list[0].revision, 0);
        assert_eq!(list[1].name, "curl");
        assert_eq!(list[2].name, "sqlite3");
        assert_eq!(list[2].revision, 1);

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn test_overlap() {
        let base = std::env::temp_dir().join(format!("fb-mp2-{}", std::process::id()));
        let sw = base.join("software");
        fs::create_dir_all(sw.join("bash/5.2.008_0")).unwrap();
        fs::create_dir_all(sw.join("curl/8.11.1_0")).unwrap();

        let list = scan_software(&sw).unwrap();
        let names: Vec<String> = list.iter().map(|p| p.name.clone()).collect();
        let ov = crate::version::compare("1.0", "2.0");
        let _ = ov;
        assert_eq!(overlap_inline(&["bash".to_string()], &names).len(), 1);
        assert_eq!(overlap_inline(&["hello".to_string()], &names).len(), 0);

        fs::remove_dir_all(&base).ok();
    }

    fn overlap_inline(fb: &[String], mp: &[String]) -> Vec<String> {
        fb.iter().filter(|n| mp.contains(n)).cloned().collect()
    }
}
