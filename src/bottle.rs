use crate::archive;
use crate::config::Config;
use crate::download;
use crate::formula::{Bottle, Formula};
use crate::platform;
use anyhow::{anyhow, bail, Context, Result};
use ed25519_dalek::{Signature, VerifyingKey};
use std::path::{Path, PathBuf};

/// 一个 bottle 候选：按标签挑选后的下载地址与校验信息。
#[derive(Debug, Clone)]
pub struct Candidate {
    pub label: String,
    /// 候选下载地址，镜像在前、主地址在后
    pub urls: Vec<String>,
    pub sha256: String,
    pub signature: String,
    pub public_key: String,
}

/// 选择当前平台可用的 bottle 候选：平台专用标签优先，其次通用 bottle。
/// 没有可用候选时返回空列表（调用方回退源码构建）。
pub fn candidates(f: &Formula) -> Vec<Candidate> {
    let Some(b) = f.bottle.as_ref() else {
        return Vec::new();
    };
    select(b, &f.package.name, platform::macos_tag())
}

fn select(b: &Bottle, name: &str, current_tag: Option<&str>) -> Vec<Candidate> {
    let mut out = Vec::new();

    if let Some(tag) = current_tag {
        if let Some(t) = b.tags.get(tag) {
            if arch_ok(&t.arch) && !looks_non_macos(&t.url) {
                let c = candidate_from(
                    tag,
                    &t.url,
                    &t.sha256,
                    &t.mirrors,
                    &t.signature,
                    &t.public_key,
                );
                if !c.urls.is_empty() {
                    out.push(c);
                }
            } else {
                eprintln!(
                    "警告: {name} 的 {tag} bottle 架构为 `{}`，非 Intel，已忽略",
                    t.arch
                );
            }
        }
    }

    if arch_ok(&b.arch) {
        if platform_ok(b, current_tag) {
            let c = candidate_from(
                "默认",
                &b.url,
                &b.sha256,
                &b.mirrors,
                &b.signature,
                &b.public_key,
            );
            if !c.urls.is_empty() {
                out.push(c);
            }
        } else {
            if b.platforms.is_empty() {
                eprintln!(
                    "警告: {name} 的 bottle 产物 `{}` 不是 macOS 平台，已忽略",
                    b.url
                );
            } else {
                eprintln!(
                    "警告: {name} 的 bottle 仅适用于 {:?}，当前平台为 {}，已忽略",
                    b.platforms,
                    current_tag.unwrap_or("未知")
                );
            }
        }
    } else {
        eprintln!(
            "警告: {name} 的 bottle 架构为 `{}`，非 Intel，已忽略",
            b.arch
        );
    }

    out
}

fn candidate_from(
    label: &str,
    url: &str,
    sha256: &str,
    mirrors: &[String],
    signature: &str,
    public_key: &str,
) -> Candidate {
    let mut urls: Vec<String> = Vec::new();
    for m in mirrors {
        push_unique(&mut urls, m);
    }
    push_unique(&mut urls, url);
    Candidate {
        label: label.to_string(),
        urls,
        sha256: sha256.trim().to_string(),
        signature: signature.trim().to_string(),
        public_key: public_key.trim().to_string(),
    }
}

fn push_unique(urls: &mut Vec<String>, url: &str) {
    let u = url.trim();
    if !u.is_empty() && !urls.iter().any(|x| x == u) {
        urls.push(u.to_string());
    }
}

fn arch_ok(arch: &str) -> bool {
    match arch.trim().to_ascii_lowercase().as_str() {
        "" | "x86_64" | "x86-64" | "amd64" | "intel" => true,
        _ => false,
    }
}

fn normalize_platform(p: &str) -> String {
    p.trim().to_ascii_lowercase().replace('-', "_")
}

/// bottle 产物是否明显不是 macOS（文件名/路径里带 linux/freebsd/windows 之类标记）。
/// Intel Mac 只能安装 x86_64 macOS 产物；只凭 arch 字段无法区分 Linux 与 macOS，
/// 因此平台标签缺省时用产物路径兜底排除。
fn looks_non_macos(url: &str) -> bool {
    let u = url.trim().to_ascii_lowercase();
    [
        "linux", "freebsd", "openbsd", "netbsd", "windows", "_win", ".dll", ".exe",
    ]
    .iter()
    .any(|m| u.contains(m))
}

/// 平台校验：显式 platforms 必须命中当前 macOS 标签；platforms 缺省时拒绝
/// 路径里带非 macOS 标记的产物。返回 false 表示该 bottle 不该在 Intel Mac 上安装。
fn platform_ok(b: &Bottle, current_tag: Option<&str>) -> bool {
    if b.platforms.is_empty() {
        return !looks_non_macos(&b.url);
    }
    match current_tag {
        Some(tag) => b
            .platforms
            .iter()
            .any(|p| normalize_platform(p) == tag),
        None => false,
    }
}

/// 按候选顺序下载并落盘 bottle。返回 false 表示没有可用 bottle 或全部失败，
/// 由调用方回退源码构建。
pub fn pour(cfg: &Config, f: &Formula, work: &Path) -> Result<bool> {
    let list = candidates(f);
    if list.is_empty() {
        return Ok(false);
    }

    let archive = work.join("bottle.tar.gz");
    let dest = work.join("bottle");

    for c in &list {
        println!("    尝试 bottle [{}]", c.label);
        for url in &c.urls {
            println!("    下载 {url}");
            match pour_from(cfg, f, work, url, c, &archive, &dest) {
                Ok(()) => {
                    println!("    bottle 安装成功 [{}]", c.label);
                    return Ok(true);
                }
                Err(e) => {
                    eprintln!("警告: bottle 候选失败（{url}）: {e:#}");
                    let _ = std::fs::remove_file(&archive);
                    let _ = std::fs::remove_dir_all(&dest);
                }
            }
        }
        let _ = std::fs::remove_file(&archive);
        let _ = std::fs::remove_dir_all(&dest);
    }

    eprintln!("警告: 所有 bottle 候选均不可用，回退源码构建");
    Ok(false)
}

fn pour_from(
    cfg: &Config,
    f: &Formula,
    work: &Path,
    url: &str,
    c: &Candidate,
    archive: &Path,
    dest: &Path,
) -> Result<()> {
    download::fetch(url, archive)?;
    download::verify(archive, &c.sha256, &f.package.name)?;
    if !c.signature.is_empty() {
        let sig_path = fetch_signature(work, &c.signature)?;
        verify_signature(archive, &sig_path, &c.public_key)
            .with_context(|| format!("{} bottle 签名校验失败", f.package.name))?;
        println!("    签名校验通过");
    }

    archive::extract(archive, archive::detect(url)?, dest)?;
    let root = resolve_bottle_root(dest, &f.package.name, &f.package.version);
    let keg = f.cellar_dir(&cfg.prefix);
    if keg.exists() {
        std::fs::remove_dir_all(&keg)
            .with_context(|| format!("清理旧的 keg 失败: {}", keg.display()))?;
    }
    if let Some(parent) = keg.parent() {
        std::fs::create_dir_all(parent)?;
    }
    move_tree(&root, &keg)?;

    let relocated = relocate(cfg, &keg)?;
    if relocated > 0 && cfg.verbose {
        println!("    重定位 {relocated} 个文件");
    }
    Ok(())
}

/// 判定 bottle 解包后的 keg 根目录。兼容三种布局：
/// 1. `<version>/...`（archive 已剥离唯一的 <name> 顶层）
/// 2. `<name>/<version>/...`（未剥离）
/// 3. 归档内容本身就是 keg（`bin/...` 等）
pub fn resolve_bottle_root(dest: &Path, name: &str, version: &str) -> PathBuf {
    let a = dest.join(version);
    if a.is_dir() {
        return a;
    }
    let b = dest.join(name).join(version);
    if b.is_dir() {
        return b;
    }
    dest.to_path_buf()
}

fn fetch_signature(work: &Path, spec: &str) -> Result<PathBuf> {
    if spec.starts_with("http://") || spec.starts_with("https://") {
        let p = work.join("bottle.sig");
        download::fetch(spec, &p)?;
        Ok(p)
    } else {
        let p = PathBuf::from(spec);
        if !p.exists() {
            bail!("签名文件不存在: {}", p.display());
        }
        Ok(p)
    }
}

/// 校验分离式 Ed25519 签名。签名文件可为 64 字节原始数据或 hex 文本；
/// 公钥为 32 字节 hex。
pub fn verify_signature(data: &Path, signature_file: &Path, public_key: &str) -> Result<()> {
    if public_key.trim().is_empty() {
        bail!("配置了 signature 但缺少 public_key");
    }
    let pk = hex::decode(public_key.trim()).context("public_key 不是合法 hex")?;
    let pk_arr: [u8; 32] = pk
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("Ed25519 公钥应为 32 字节，实际 {} 字节", pk.len()))?;
    let vk = VerifyingKey::from_bytes(&pk_arr).context("无效的 Ed25519 公钥")?;

    let raw = std::fs::read(signature_file)
        .with_context(|| format!("读取签名失败: {}", signature_file.display()))?;
    let sig_bytes = if raw.len() == 64 {
        raw
    } else {
        let text = String::from_utf8_lossy(&raw).trim().to_string();
        hex::decode(&text).context("签名文件既非 64 字节原始数据，也不是合法 hex")?
    };
    let sig_arr: [u8; 64] = sig_bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("Ed25519 签名应为 64 字节，实际 {} 字节", sig_bytes.len()))?;
    let sig = Signature::from_bytes(&sig_arr);

    let data_bytes =
        std::fs::read(data).with_context(|| format!("读取被签名文件失败: {}", data.display()))?;
    vk.verify_strict(&data_bytes, &sig)
        .map_err(|e| anyhow!("Ed25519 签名不匹配（文件被篡改或签名错误）: {e}"))?;
    Ok(())
}

fn move_tree(from: &Path, to: &Path) -> Result<()> {
    if from == to {
        return Ok(());
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    copy_tree(from, to)?;
    std::fs::remove_dir_all(from)?;
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)?.flatten() {
        let src = entry.path();
        let dst = to.join(entry.file_name());
        let ft = entry.file_type()?;
        if ft.is_dir() {
            copy_tree(&src, &dst)?;
        } else if ft.is_symlink() {
            let target = std::fs::read_link(&src)?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

const MAX_TEXT_SIZE: u64 = 2 * 1024 * 1024;

/// 把 bottle 内的 Homebrew 前缀重定位到 Fieldsbrew prefix：
/// - 符号链接：改写指向这些前缀的绝对目标
/// - Mach-O：用 install_name_tool 改 install name、依赖与 rpath
/// - 文本文件：直接替换前缀字符串
pub fn relocate(cfg: &Config, keg: &Path) -> Result<usize> {
    let prefix = cfg.prefix.display().to_string();
    let mut changed = 0;
    walk_relocate(keg, &prefix, &mut changed)?;
    Ok(changed)
}

fn walk_relocate(dir: &Path, prefix: &str, changed: &mut usize) -> Result<()> {
    for entry in std::fs::read_dir(dir)?.flatten() {
        let path = entry.path();
        let ft = entry.file_type()?;

        if ft.is_symlink() {
            let target = std::fs::read_link(&path)?;
            let t = target.to_string_lossy();
            if has_old_prefix(&t) {
                let new = replace_prefixes(&t, prefix);
                std::fs::remove_file(&path)
                    .with_context(|| format!("移除旧符号链接失败: {}", path.display()))?;
                #[cfg(unix)]
                std::os::unix::fs::symlink(&new, &path)
                    .with_context(|| format!("重建符号链接失败: {}", path.display()))?;
                *changed += 1;
            }
            continue;
        }

        if ft.is_dir() {
            walk_relocate(&path, prefix, changed)?;
            continue;
        }
        if !ft.is_file() {
            continue;
        }

        if is_macho(&path)? {
            if relocate_macho(&path, prefix)? {
                *changed += 1;
            }
            continue;
        }

        let meta = entry.metadata()?;
        if meta.len() > MAX_TEXT_SIZE {
            continue;
        }
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if bytes.iter().any(|b| *b == 0) {
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        if !has_old_prefix(&text) {
            continue;
        }
        let new = replace_prefixes(&text, prefix);
        std::fs::write(&path, new)?;
        *changed += 1;
    }
    Ok(())
}

fn has_old_prefix(s: &str) -> bool {
    s.contains("/usr/local") || s.contains("/opt/homebrew") || s.contains("/home/linuxbrew")
}

fn replace_prefixes(s: &str, prefix: &str) -> String {
    s.replace("/usr/local", prefix)
        .replace("/opt/homebrew", prefix)
        .replace("/home/linuxbrew/.linuxbrew", prefix)
}

/// 通过魔数判断 Mach-O（含 fat/universal 二进制）
fn is_macho(path: &Path) -> Result<bool> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut magic = [0u8; 4];
    if f.read_exact(&mut magic).is_err() {
        return Ok(false);
    }
    Ok(matches!(
        magic,
        [0xcf, 0xfa, 0xed, 0xfe] // 64-bit little-endian
            | [0xce, 0xfa, 0xed, 0xfe] // 32-bit little-endian
            | [0xca, 0xfe, 0xba, 0xbe] // fat big-endian
            | [0xbe, 0xba, 0xfe, 0xca] // fat little-endian
            | [0xbf, 0xba, 0xfe, 0xca] // fat64 little-endian
            | [0xfe, 0xed, 0xfa, 0xcf] // 64-bit big-endian
            | [0xfe, 0xed, 0xfa, 0xce] // 32-bit big-endian
    ))
}

fn relocate_macho(path: &Path, prefix: &str) -> Result<bool> {
    let out = match std::process::Command::new("otool").arg("-L").arg(path).output() {
        Ok(o) if o.status.success() => o,
        _ => {
            eprintln!("警告: otool 无法读取 {}，跳过 Mach-O 重定位", path.display());
            return Ok(false);
        }
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let deps: Vec<String> = text
        .lines()
        .skip(1)
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.ends_with(':'))
        .map(|l| l.split(" (compatibility").next().unwrap_or(l).trim().to_string())
        .collect();
    if deps.is_empty() {
        return Ok(false);
    }

    let is_dylib = path.extension().map(|e| e == "dylib").unwrap_or(false);
    let mut changed = false;
    for (i, dep) in deps.iter().enumerate() {
        if !has_old_prefix(dep) {
            continue;
        }
        let new = replace_prefixes(dep, prefix);
        if is_dylib && i == 0 {
            run_install_name_tool(&["-id", &new], path)?;
        } else {
            run_install_name_tool(&["-change", dep, &new], path)?;
        }
        changed = true;
    }

    // rpath 同样可能硬编码 Homebrew 前缀
    if relocate_rpaths(path, prefix)? {
        changed = true;
    }
    Ok(changed)
}

fn relocate_rpaths(path: &Path, prefix: &str) -> Result<bool> {
    let out = match std::process::Command::new("otool").arg("-l").arg(path).output() {
        Ok(o) if o.status.success() => o,
        _ => return Ok(false),
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut last_cmd = String::new();
    let mut rpaths = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if let Some(cmd) = t.strip_prefix("cmd ") {
            last_cmd = cmd.to_string();
        } else if last_cmd == "LC_RPATH" {
            if let Some(p) = t.strip_prefix("path ") {
                let p = p.split(" (offset").next().unwrap_or(p).trim();
                rpaths.push(p.to_string());
            }
        }
    }
    let mut changed = false;
    for rp in rpaths {
        if !has_old_prefix(&rp) {
            continue;
        }
        let new = replace_prefixes(&rp, prefix);
        run_install_name_tool(&["-rpath", &rp, &new], path)?;
        changed = true;
    }
    Ok(changed)
}

fn run_install_name_tool(args: &[&str], path: &Path) -> Result<()> {
    let status = std::process::Command::new("install_name_tool")
        .args(args)
        .arg(path)
        .status();
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => {
            eprintln!(
                "警告: install_name_tool {:?} 失败（退出码 {}）: {}",
                args,
                s.code().unwrap_or(-1),
                path.display()
            );
            Ok(())
        }
        Err(e) => {
            eprintln!("警告: 无法运行 install_name_tool（{e}），跳过: {}", path.display());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formula::Formula;

    fn sample(bottle_toml: &str) -> Formula {
        Formula::parse(&format!(
            r#"
[package]
name = "hello"
version = "2.12.1"

[source]
url = "https://example.com/hello.tar.gz"

{bottle_toml}
"#
        ))
        .unwrap()
    }

    #[test]
    fn test_bottle_root_layouts() {
        let base = std::env::temp_dir().join(format!("fb-bottle-{}", std::process::id()));
        std::fs::create_dir_all(base.join("hello/2.12.1/bin")).unwrap();
        assert_eq!(
            resolve_bottle_root(&base, "hello", "2.12.1"),
            base.join("hello/2.12.1")
        );
        std::fs::create_dir_all(base.join("3.1/bin")).unwrap();
        assert_eq!(
            resolve_bottle_root(&base, "hello", "3.1"),
            base.join("3.1")
        );
        std::fs::create_dir_all(base.join("bin")).unwrap();
        assert_eq!(
            resolve_bottle_root(&base, "hello", "9.9"),
            PathBuf::from(&base)
        );
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn test_select_prefers_platform_tag_and_mirrors() {
        let f = sample(
            r#"
[bottle]
url = "https://example.com/generic.tar.gz"
sha256 = "g"
mirrors = ["https://mirror.example.com/generic.tar.gz"]

[bottle.tags.monterey]
url = "https://example.com/monterey.tar.gz"
sha256 = "m"
mirrors = ["https://mirror.example.com/monterey.tar.gz"]
"#,
        );
        let list = select(f.bottle.as_ref().unwrap(), "hello", Some("monterey"));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].label, "monterey");
        assert_eq!(
            list[0].urls,
            vec![
                "https://mirror.example.com/monterey.tar.gz",
                "https://example.com/monterey.tar.gz"
            ]
        );
        assert_eq!(list[1].label, "默认");
    }

    #[test]
    fn test_select_rejects_non_intel_and_wrong_platform() {
        let f = sample(
            r#"
[bottle]
url = "https://example.com/arm.tar.gz"
sha256 = "a"
arch = "arm64"

[bottle.tags.ventura]
url = "https://example.com/arm64-ventura.tar.gz"
sha256 = "v"
arch = "arm64"
"#,
        );
        let list = select(f.bottle.as_ref().unwrap(), "hello", Some("ventura"));
        assert!(list.is_empty());

        let f = sample(
            r#"
[bottle]
url = "https://example.com/x86.tar.gz"
sha256 = "x"
platforms = ["catalina", "big-sur"]
"#,
        );
        assert!(select(f.bottle.as_ref().unwrap(), "hello", Some("ventura")).is_empty());
        assert_eq!(
            select(f.bottle.as_ref().unwrap(), "hello", Some("catalina")).len(),
            1
        );
    }

    #[test]
    fn test_relocate_text_and_symlink() {
        let dir = std::env::temp_dir().join(format!("fb-reloc-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(
            dir.join("bin/tool"),
            "#!/bin/sh\nexec /usr/local/opt/hello/bin/hello \"$@\"\n",
        )
        .unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/usr/local/opt/hello/lib", dir.join("lib")).unwrap();

        let cfg = Config {
            prefix: PathBuf::from("/opt/fieldsbrew"),
            verbose: false,
        };
        let changed = relocate(&cfg, &dir).unwrap();
        assert!(changed >= 2);
        let text = std::fs::read_to_string(dir.join("bin/tool")).unwrap();
        assert!(text.contains("/opt/fieldsbrew/opt/hello/bin/hello"));
        assert!(!text.contains("/usr/local"));
        #[cfg(unix)]
        assert_eq!(
            std::fs::read_link(dir.join("lib")).unwrap(),
            PathBuf::from("/opt/fieldsbrew/opt/hello/lib")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_verify_signature() {
        use ed25519_dalek::{Signer, SigningKey};

        let key = SigningKey::from_bytes(&[7u8; 32]);
        let data = b"bottle contents";
        let sig = key.sign(data);
        let dir = std::env::temp_dir().join(format!("fb-sig-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let data_path = dir.join("bottle.tar.gz");
        let sig_path = dir.join("bottle.sig");
        std::fs::write(&data_path, data).unwrap();
        std::fs::write(&sig_path, hex::encode(sig.to_bytes())).unwrap();
        let pk = hex::encode(key.verifying_key().to_bytes());

        verify_signature(&data_path, &sig_path, &pk).unwrap();

        std::fs::write(&data_path, b"tampered").unwrap();
        assert!(verify_signature(&data_path, &sig_path, &pk).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    fn bottle_with(url: &str, platforms: &[&str]) -> Bottle {
        Bottle {
            url: url.to_string(),
            platforms: platforms.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn test_reject_non_macos_bottle() {
        let b = bottle_with(
            "https://example.com/xz%405.8.4.x86_64_linux.bottled.tar.gz",
            &[],
        );
        assert!(looks_non_macos(&b.url));
        assert!(!platform_ok(&b, Some("sonoma")));
    }

    #[test]
    fn test_accept_macos_bottle_without_tag() {
        let b = bottle_with("https://example.com/hello-2.12.1.sonoma.bottle.tar.gz", &[]);
        assert!(!looks_non_macos(&b.url));
        assert!(platform_ok(&b, Some("sonoma")));
    }

    #[test]
    fn test_explicit_platform_must_match() {
        let b = bottle_with("https://example.com/hello.bottle.tar.gz", &["ventura"]);
        assert!(!platform_ok(&b, Some("sonoma")));
        assert!(platform_ok(&b, Some("ventura")));
    }
}
