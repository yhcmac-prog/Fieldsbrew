use anyhow::Result;
use std::collections::BTreeMap;
use std::fmt::Write;

/// 支持的 Homebrew DSL 子集：把 Ruby formula / cask 转成 Fieldsbrew TOML。
/// 转换只做静态文本分析，不执行 Ruby；无法表达的语义会以 warnings 返回，由人工复核。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Formula,
    Cask,
}

#[derive(Debug, Clone)]
pub struct Converted {
    pub kind: Kind,
    pub name: String,
    pub toml: String,
    pub warnings: Vec<String>,
}

pub fn detect_kind(src: &str) -> Kind {
    for line in src.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t.starts_with("cask ") {
            return Kind::Cask;
        }
        if t.starts_with("class ") {
            return Kind::Formula;
        }
    }
    Kind::Formula
}

pub fn convert(src: &str, file_stem: &str) -> Result<Converted> {
    convert_as(src, file_stem, detect_kind(src))
}

/// 按指定类型转换（`fb convert --cask` 可强制按 cask 解析）
pub fn convert_as(src: &str, file_stem: &str, kind: Kind) -> Result<Converted> {
    match kind {
        Kind::Cask => convert_cask(src, file_stem),
        Kind::Formula => convert_formula(src, file_stem),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ctx {
    Other,
    Bottle,
    Service,
    Install,
    Test,
    Ignore,
}

/// 判断当前行是否开启一个 Ruby 块，并归类上下文。
fn opener(line: &str) -> Option<Ctx> {
    let t = line.trim();
    let is_do = t.ends_with(" do");
    let is_kw = t.starts_with("if ")
        || t.starts_with("unless ")
        || t.starts_with("case ")
        || t.starts_with("while ")
        || t.starts_with("until ")
        || t.starts_with("begin")
        || t.starts_with("class ")
        || t.starts_with("module ")
        || t.starts_with("def ");
    if !(is_do || is_kw) {
        return None;
    }
    if t.starts_with("on_arm") || t.starts_with("on_linux") {
        return Some(Ctx::Ignore);
    }
    if t.starts_with("bottle ") && is_do {
        return Some(Ctx::Bottle);
    }
    if t.starts_with("service ") && is_do {
        return Some(Ctx::Service);
    }
    if t.starts_with("install do") || t.starts_with("def install") {
        return Some(Ctx::Install);
    }
    if t.starts_with("test do") || t.starts_with("def test") {
        return Some(Ctx::Test);
    }
    Some(Ctx::Other)
}

fn current_ctx(stack: &[Ctx]) -> Option<Ctx> {
    stack.iter().rev().copied().find(|c| !matches!(c, Ctx::Other))
}

/// 提取第一个引号字符串，返回内容与结束位置（字节偏移）。
fn take_string(s: &str) -> Option<(String, usize)> {
    let start = s.find(|c| c == '"' || c == '\'')?;
    let quote = s[start..].chars().next()?;
    let mut out = String::new();
    let mut escaped = false;
    for (off, ch) in s[start + 1..].char_indices() {
        let idx = start + 1 + off;
        if escaped {
            out.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && quote == '"' {
            escaped = true;
            continue;
        }
        if ch == quote {
            return Some((out, idx + ch.len_utf8()));
        }
        out.push(ch);
    }
    None
}

fn symbol_name(s: &str) -> Option<String> {
    let rest = s.trim().strip_prefix(':')?;
    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn split_args(inner: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in inner.chars() {
        if let Some(q) = quote {
            cur.push(ch);
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' && q == '"' {
                escaped = true;
                continue;
            }
            if ch == q {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => {
                quote = Some(ch);
                cur.push(ch);
            }
            ',' => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// 把 service run 中的 Ruby 表达式转换为 prefix 相对路径。
fn path_expr(expr: &str, name: &str) -> String {
    let e = expr.trim().trim_end_matches(',').trim();
    for (prefix, repl) in [
        ("opt_bin/", format!("opt/{name}/bin/")),
        ("opt_prefix/", format!("opt/{name}/")),
        ("opt_libexec/", format!("opt/{name}/libexec/")),
        ("bin/", format!("opt/{name}/bin/")),
        ("libexec/", format!("opt/{name}/libexec/")),
        ("var/", "var/".to_string()),
        ("etc/", "etc/".to_string()),
    ] {
        if let Some(rest) = e.strip_prefix(prefix) {
            if let Some((v, _)) = take_string(rest) {
                return format!("{repl}{v}");
            }
        }
    }
    if let Some((s, _)) = take_string(e) {
        s
    } else {
        e.to_string()
    }
}

fn command_expr(rest: &str, name: &str) -> String {
    let t = rest.trim();
    if let Some(inner) = t.strip_prefix('[').and_then(|x| x.strip_suffix(']')) {
        split_args(inner)
            .iter()
            .map(|a| path_expr(a, name))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        path_expr(t, name)
    }
}

fn env_pairs(rest: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for part in split_args(rest) {
        if let Some(i) = part.find(':') {
            let key = part[..i].trim().trim_matches('"').to_string();
            if let Some((v, _)) = take_string(&part[i + 1..]) {
                if !key.is_empty() {
                    out.push((key, v));
                }
            }
        }
    }
    out
}

#[derive(Debug, Default)]
struct ServiceOut {
    run: String,
    keep_alive: bool,
    run_at_load: bool,
    working_dir: String,
    log_path: String,
    error_log_path: String,
    env: BTreeMap<String, String>,
}

fn tomllib_quote(s: &str) -> String {
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn bottle_sha(line: &str) -> Option<(String, Option<String>)> {
    let rest = line.trim().strip_prefix("sha256")?.trim_start();
    let q = rest.find('"')?;
    let prefix = rest[..q].trim();
    if let Some(tag) = prefix.strip_suffix(':') {
        let tag = tag.trim();
        let (sha, _) = take_string(rest)?;
        return Some((sha, Some(tag.to_string())));
    }
    if prefix.is_empty() {
        let (sha, after) = take_string(rest)?;
        let tail = &rest[after..];
        let tag = tail
            .find("=>")
            .and_then(|i| symbol_name(&tail[i + 2..]));
        return Some((sha, tag));
    }
    None
}

fn parse_version_revision(version: &str, revision: Option<u32>) -> String {
    match revision {
        Some(r) if r > 0 && !version.contains('_') => format!("{version}_{r}"),
        _ => version.to_string(),
    }
}

/// 从源码 URL 推断版本号（Homebrew formula 常省略 version）
fn infer_version(url: &str, name: &str) -> Option<String> {
    let path = url.split(['?', '#']).next()?;
    let mut stem = path.rsplit('/').next().filter(|s| !s.is_empty())?;
    for ext in [
        ".tar.gz", ".tgz", ".tar.xz", ".txz", ".tar.bz2", ".tbz2", ".tar", ".zip", ".dmg", ".pkg",
    ] {
        if let Some(s) = stem.strip_suffix(ext) {
            stem = s;
            break;
        }
    }
    let stripped = stem
        .strip_prefix(&format!("{name}-"))
        .or_else(|| stem.strip_prefix(&format!("{name}_")))
        .unwrap_or(stem);
    let v = stripped.trim_start_matches('v');
    if v.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
        Some(v.to_string())
    } else {
        None
    }
}

fn supported_bottle_tag(tag: &str) -> Option<&str> {
    let t = tag.trim();
    if t.contains("arm64") || t.contains("linux") || t.contains("x86_64_linux") {
        return None;
    }
    let t = t.strip_prefix("x86_64_").unwrap_or(t);
    match t {
        "catalina" | "big_sur" | "monterey" | "ventura" | "sonoma" | "sequoia" | "tahoe" => Some(t),
        _ => None,
    }
}

fn convert_formula(src: &str, file_stem: &str) -> Result<Converted> {
    let name = if file_stem.is_empty() {
        "formula".to_string()
    } else {
        file_stem.to_string()
    };
    let mut warnings = Vec::new();
    let mut version = String::new();
    let mut desc = String::new();
    let mut homepage = String::new();
    let mut source_url = String::new();
    let mut source_sha = String::new();
    let mut dependencies: Vec<String> = Vec::new();
    let mut dep_names: Vec<String> = Vec::new();
    let mut keg_only = false;
    let mut revision: Option<u32> = None;
    let mut bottle_root = String::new();
    let mut bottle_tags: Vec<(String, String)> = Vec::new();
    let mut seen_bottle = false;
    let mut build_system = String::new();
    let mut warned_meson = false;
    let mut service = ServiceOut::default();
    let mut stack: Vec<Ctx> = Vec::new();

    for line in src.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t == "end" {
            stack.pop();
            continue;
        }
        let ctx = current_ctx(&stack);
        let ignored = stack.iter().any(|c| *c == Ctx::Ignore) || ctx == Some(Ctx::Ignore);
        if let Some(c) = opener(t) {
            stack.push(c);
        }
        if ignored {
            continue;
        }
        match ctx {
            Some(Ctx::Bottle) => {
                if t.starts_with("root_url") {
                    if let Some((v, _)) = take_string(t) {
                        bottle_root = v;
                    }
                } else if t.starts_with("sha256") {
                    if let Some((sha, tag)) = bottle_sha(t) {
                        match tag {
                            Some(tag) => match supported_bottle_tag(&tag) {
                                Some(tag) => {
                                    if !bottle_tags.iter().any(|(x, _)| x == tag) {
                                        bottle_tags.push((tag.to_string(), sha));
                                    }
                                }
                                None => {}
                            },
                            None => warnings.push("bottle 中的 sha256 缺少平台标签，已跳过".to_string()),
                        }
                    }
                }
                continue;
            }
            Some(Ctx::Service) => {
                if t.starts_with("run ") {
                    service.run = command_expr(&t[4..], &name);
                } else if t.starts_with("keep_alive") {
                    service.keep_alive = t.contains("true");
                } else if t.starts_with("run_at_load") {
                    service.run_at_load = t.contains("true");
                } else if t.starts_with("working_dir") {
                    service.working_dir = path_expr(&t["working_dir".len()..], &name);
                } else if t.starts_with("log_path") {
                    service.log_path = path_expr(&t["log_path".len()..], &name);
                } else if t.starts_with("error_log_path") {
                    service.error_log_path = path_expr(&t["error_log_path".len()..], &name);
                } else if t.starts_with("environment_variables") {
                    for (k, v) in env_pairs(&t["environment_variables".len()..]) {
                        service.env.insert(k, v);
                    }
                }
                continue;
            }
            Some(Ctx::Install) => {
                if t.contains("meson") && !warned_meson {
                    warnings.push("检测到 meson 构建，当前仅支持 autotools/cmake/make，请人工调整".to_string());
                    warned_meson = true;
                }
                if t.contains("cmake") {
                    build_system = "cmake".to_string();
                } else if build_system.is_empty()
                    && (t.contains("PREFIX=") || t.contains("PREFIX=#{prefix}"))
                {
                    build_system = "make".to_string();
                }
                continue;
            }
            Some(Ctx::Test) => continue,
            _ => {}
        }

        // 顶层与普通块（含 on_intel）中的声明
        if t.starts_with("version ") && version.is_empty() {
            version = take_string(t)
                .map(|(v, _)| v)
                .or_else(|| symbol_name(&t["version".len()..]))
                .unwrap_or_default();
        } else if (t.starts_with("desc ") || t.starts_with("description ")) && desc.is_empty() {
            desc = take_string(t).map(|(v, _)| v).unwrap_or_default();
        } else if t.starts_with("homepage ") && homepage.is_empty() {
            homepage = take_string(t).map(|(v, _)| v).unwrap_or_default();
        } else if t.starts_with("url ") && source_url.is_empty() {
            if let Some((v, after)) = take_string(t) {
                let tail = &t[after..];
                if tail.contains("using: :git")
                    || tail.contains("using: :hg")
                    || tail.contains("using: :svn")
                {
                    warnings.push(format!(
                        "源码使用版本控制系统（{v}），Fieldsbrew 仅支持归档下载，请人工确认"
                    ));
                }
                source_url = v;
            }
        } else if t.starts_with("sha256") && !seen_bottle && source_sha.is_empty() {
            if let Some((v, _)) = take_string(t) {
                source_sha = v;
            } else if t.contains(":no_check") {
                warnings.push("源码 sha256 为 :no_check，转换后为空，需自行补全".to_string());
            }
        } else if t.starts_with("mirror ") {
            warnings.push("源码 mirror 暂不支持，已忽略".to_string());
        } else if t.starts_with("depends_on") {
            // `depends_on :xcode => ["16.0", :build]` 这类符号依赖描述的是宿主工具链
            // 或系统版本，不是可安装的 formula。take_string 会抓到 `=>` 后面的第一个
            // 字符串，直接把版本号当成包名，因此这里必须先拦下来。
            if t["depends_on".len()..].trim_start().starts_with(':') {
                match symbol_name(&t["depends_on".len()..]) {
                    Some(sym) => warnings.push(format!("depends_on :{sym} 为系统/工具链依赖，未转换")),
                    None => warnings.push("depends_on 的符号依赖未转换".to_string()),
                }
            } else if let Some(dep) = parse_dependency(t) {
                let dep_name = dep
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_string();
                if !dep_names.contains(&dep_name) {
                    dep_names.push(dep_name);
                    dependencies.push(dep);
                }
            }
        } else if t.starts_with("uses_from_macos") {
            warnings.push("uses_from_macos 依赖未转换（通常由系统提供）".to_string());
        } else if t.starts_with("keg_only") {
            keg_only = true;
        } else if t.starts_with("revision ") {
            revision = t["revision".len()..].trim().parse().ok();
        } else if t.starts_with("bottle") {
            if t.starts_with("bottle do") {
                seen_bottle = true;
            }
        }
    }

    if version.is_empty() {
        // Homebrew 通常不写 version，而是从源码 URL 文件名推断
        match infer_version(&source_url, &name) {
            Some(v) => version = v,
            None => {
                version = "0".to_string();
                warnings.push("未找到 version 且无法从 URL 推断，已使用 0 占位".to_string());
            }
        }
    }
    let full_version = parse_version_revision(&version, revision);

    // 处理常见的 Ruby 插值
    let expand = |s: &str| -> String {
        s.replace("#{version}", &full_version)
            .replace("#{name}", &name)
    };
    source_url = expand(&source_url);
    bottle_root = expand(&bottle_root);

    if seen_bottle && bottle_tags.is_empty() {
        warnings.push("bottle 块中没有可用的 Intel 平台标签，未生成 bottle 配置".to_string());
    }
    if !bottle_tags.is_empty() && bottle_root.is_empty() {
        warnings.push(
            "bottle 块缺少 root_url，未生成下载 URL，请人工补全 [bottle.tags.*].url".to_string(),
        );
    }
    if bottle_root.contains("#{") {
        warnings.push(format!(
            "bottle root_url 含未解析插值，请人工补全: {bottle_root}"
        ));
    }

    let mut out = String::new();
    let _ = writeln!(
        out,
        "# 由 `fb convert` 从 Homebrew formula 自动生成；URL、构建参数与依赖请复核。"
    );
    if !dependencies.is_empty() {
        let list = dependencies
            .iter()
            .map(|d| tomllib_quote(d))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(out, "dependencies = [{list}]");
        let _ = writeln!(out);
    }
    let _ = writeln!(out, "[package]");
    let _ = writeln!(out, "name = {}", tomllib_quote(&name));
    let _ = writeln!(out, "version = {}", tomllib_quote(&full_version));
    let _ = writeln!(out, "description = {}", tomllib_quote(&desc));
    let _ = writeln!(out, "homepage = {}", tomllib_quote(&homepage));
    let _ = writeln!(out);
    let _ = writeln!(out, "[source]");
    let _ = writeln!(out, "url = {}", tomllib_quote(&source_url));
    if source_sha.is_empty() {
        let _ = writeln!(out, "# sha256 未找到，安装时将跳过完整性校验");
    }
    let _ = writeln!(out, "sha256 = {}", tomllib_quote(&source_sha));
    if keg_only {
        let _ = writeln!(out);
        let _ = writeln!(out, "[install]");
        let _ = writeln!(out, "keg_only = true");
    }
    if !bottle_tags.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "[bottle]");
        let _ = writeln!(out, "arch = {}", tomllib_quote("x86_64"));
        bottle_tags.sort_by(|a, b| a.0.cmp(&b.0));
        for (tag, sha) in &bottle_tags {
            let _ = writeln!(out);
            let _ = writeln!(out, "[bottle.tags.{tag}]");
            let url = if bottle_root.is_empty() {
                String::new()
            } else {
                format!(
                    "{}/{}/{}/{}-{}.{}.bottle.tar.gz",
                    bottle_root.trim_end_matches('/'),
                    name,
                    full_version,
                    name,
                    full_version,
                    tag
                )
            };
            let _ = writeln!(out, "url = {}", tomllib_quote(&url));
            let _ = writeln!(out, "sha256 = {}", tomllib_quote(sha));
            let _ = writeln!(out, "arch = {}", tomllib_quote("x86_64"));
        }
    }
    if !service.run.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "[service]");
        let _ = writeln!(out, "run = {}", tomllib_quote(&service.run));
        if service.keep_alive {
            let _ = writeln!(out, "keep_alive = true");
        }
        if service.run_at_load {
            let _ = writeln!(out, "run_at_load = true");
        }
        for (key, val) in [
            ("working_dir", &service.working_dir),
            ("log_path", &service.log_path),
            ("error_log_path", &service.error_log_path),
        ] {
            if !val.is_empty() {
                let _ = writeln!(out, "{key} = {}", tomllib_quote(val));
            }
        }
        if !service.env.is_empty() {
            let _ = writeln!(out);
            let _ = writeln!(out, "[service.env]");
            for (k, v) in &service.env {
                let _ = writeln!(out, "{k} = {}", tomllib_quote(v));
            }
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "[build]");
    let system = if build_system.is_empty() {
        "autotools".to_string()
    } else {
        build_system
    };
    let _ = writeln!(out, "system = {}", tomllib_quote(&system));

    Ok(Converted {
        kind: Kind::Formula,
        name,
        toml: out,
        warnings,
    })
}

fn parse_dependency(line: &str) -> Option<String> {
    let (dep, after) = take_string(line)?;
    let tail = &line[after..];
    if let Some(i) = tail.find("=>") {
        let val = tail[i + 2..].trim();
        if let Some((req, _)) = take_string(val) {
            let req = req.trim();
            if !req.is_empty() {
                return Some(format!("{dep} {req}"));
            }
        }
    }
    Some(dep)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaskCtx {
    Root,
    Process,
    Ignore,
}

fn cask_opener(line: &str) -> Option<CaskCtx> {
    let t = line.trim();
    let is_do = t.ends_with(" do");
    if !is_do
        && !(t.starts_with("if ")
            || t.starts_with("unless ")
            || t.starts_with("case ")
            || t.starts_with("def "))
    {
        return None;
    }
    if t.starts_with("cask ") {
        return Some(CaskCtx::Root);
    }
    if t.starts_with("on_intel") {
        return Some(CaskCtx::Process);
    }
    Some(CaskCtx::Ignore)
}

fn cask_artifact(line: &str) -> Option<(String, String, String)> {
    for (kw, kind) in [
        ("app ", "app"),
        ("binary ", "binary"),
        ("font ", "font"),
        ("pkg ", "pkg"),
    ] {
        if let Some(rest) = line.strip_prefix(kw) {
            let (path, after) = take_string(rest)?;
            let tail = &rest[after..];
            let target = tail
                .find("target:")
                .and_then(|i| take_string(&tail[i + 7..]).map(|(s, _)| s))
                .unwrap_or_default();
            return Some((kind.to_string(), path, target));
        }
    }
    None
}

fn convert_cask(src: &str, file_stem: &str) -> Result<Converted> {
    let token = if file_stem.is_empty() {
        "cask".to_string()
    } else {
        file_stem.to_string()
    };
    let mut warnings = Vec::new();
    let mut name = String::new();
    let mut version = String::new();
    let mut desc = String::new();
    let mut homepage = String::new();
    let mut url = String::new();
    let mut sha = String::new();
    let mut artifacts: Vec<(String, String, String)> = Vec::new();
    let mut stack: Vec<CaskCtx> = Vec::new();

    for line in src.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t == "end" {
            stack.pop();
            continue;
        }
        let ignored = stack.iter().any(|c| *c == CaskCtx::Ignore);
        if let Some(c) = cask_opener(t) {
            stack.push(c);
        }
        if ignored {
            continue;
        }
        if t.starts_with("version ") && version.is_empty() {
            version = take_string(t)
                .map(|(v, _)| v)
                .or_else(|| symbol_name(&t["version".len()..]))
                .unwrap_or_default();
        } else if t.starts_with("name ") && name.is_empty() {
            name = take_string(t).map(|(v, _)| v).unwrap_or_default();
        } else if (t.starts_with("desc ") || t.starts_with("description ")) && desc.is_empty() {
            desc = take_string(t).map(|(v, _)| v).unwrap_or_default();
        } else if t.starts_with("homepage ") && homepage.is_empty() {
            homepage = take_string(t).map(|(v, _)| v).unwrap_or_default();
        } else if t.starts_with("url ") && url.is_empty() {
            url = take_string(t).map(|(v, _)| v).unwrap_or_default();
        } else if t.starts_with("sha256 ") && sha.is_empty() {
            if let Some((v, _)) = take_string(t) {
                sha = v;
            } else if t.contains(":no_check") {
                warnings.push("sha256 为 :no_check，转换后为空，安装时跳过校验".to_string());
            }
        } else if let Some(art) = cask_artifact(t) {
            if art.0 == "pkg" {
                warnings.push(format!(
                    "cask 包含 pkg 安装器（{}），Fieldsbrew 暂不支持 pkg，请人工处理",
                    art.1
                ));
            }
            artifacts.push(art);
        } else if t.starts_with("installer ")
            || t.starts_with("uninstall ")
            || t.starts_with("artifact ")
        {
            warnings.push(format!("未支持的 cask 指令，已忽略: {t}"));
        }
    }

    if version.is_empty() {
        version = "latest".to_string();
        warnings.push("未找到 version，已使用 latest".to_string());
    }
    if name.is_empty() {
        name = token.clone();
    }
    if url.is_empty() {
        warnings.push("未找到下载 url".to_string());
    }
    if artifacts.is_empty() {
        warnings.push("未识别到 app/binary/font 等安装产物".to_string());
    }

    let expand = |s: &str| s.replace("#{version}", &version).replace("#{token}", &token);
    let url = expand(&url);

    let mut out = String::new();
    let _ = writeln!(
        out,
        "# 由 `fb convert --cask` 从 Homebrew cask 自动生成；请复核 URL 与安装产物。"
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "[package]");
    let _ = writeln!(out, "token = {}", tomllib_quote(&token));
    let _ = writeln!(out, "name = {}", tomllib_quote(&name));
    let _ = writeln!(out, "version = {}", tomllib_quote(&version));
    let _ = writeln!(out, "description = {}", tomllib_quote(&desc));
    let _ = writeln!(out, "homepage = {}", tomllib_quote(&homepage));
    let _ = writeln!(out);
    let _ = writeln!(out, "[source]");
    let _ = writeln!(out, "url = {}", tomllib_quote(&url));
    if sha.is_empty() {
        let _ = writeln!(out, "# sha256 未找到，安装时将跳过完整性校验");
    }
    let _ = writeln!(out, "sha256 = {}", tomllib_quote(&sha));
    for (kind, path, target) in &artifacts {
        let _ = writeln!(out);
        let _ = writeln!(out, "[[artifacts]]");
        let _ = writeln!(out, "type = {}", tomllib_quote(kind));
        let _ = writeln!(out, "path = {}", tomllib_quote(path));
        if !target.is_empty() {
            let _ = writeln!(out, "target = {}", tomllib_quote(target));
        }
    }

    Ok(Converted {
        kind: Kind::Cask,
        name: token,
        toml: out,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cask::Cask;
    use crate::formula::Formula;

    const FORMULA: &str = r#"
class Hello < Formula
  desc "GNU Hello"
  homepage "https://www.gnu.org/software/hello/"
  url "https://ftp.gnu.org/gnu/hello/hello-2.12.1.tar.gz"
  sha256 "8d9914abc"
  license "GPL-3.0-or-later"
  revision 1
  depends_on "pcre2" => ">=10.0"
  depends_on "zlib"
  keg_only :versioned_formula

  bottle do
    root_url "https://example.com/bottles"
    sha256 "aaa" => :monterey
    sha256 "bbb" => :ventura
    sha256 arm64_monterey: "ccc"
  end

  service do
    run [opt_bin/"hello", "--flag"]
    keep_alive true
    log_path var/"log/hello.log"
  end

  def install
    system "./configure", "--prefix=#{prefix}"
    system "make", "install"
  end
end
"#;

    #[test]
    fn test_detect_kind() {
        assert_eq!(detect_kind(FORMULA), Kind::Formula);
        assert_eq!(
            detect_kind("cask \"demo\" do\n  version \"1\"\nend\n"),
            Kind::Cask
        );
    }

    #[test]
    fn test_convert_formula() {
        let c = convert_formula(FORMULA, "hello").unwrap();
        assert_eq!(c.kind, Kind::Formula);
        let f = Formula::parse(&c.toml).unwrap();
        assert_eq!(f.package.name, "hello");
        assert_eq!(f.package.version, "2.12.1_1");
        assert_eq!(f.package.description, "GNU Hello");
        assert_eq!(f.source.url, "https://ftp.gnu.org/gnu/hello/hello-2.12.1.tar.gz");
        assert_eq!(f.source.sha256, "8d9914abc");
        assert!(f.install.keg_only);
        let deps = f.deps().unwrap();
        assert_eq!(deps[0].name, "pcre2");
        assert!(deps[0].req.as_ref().unwrap().matches("10.42"));
        assert_eq!(deps[1].name, "zlib");
        assert_eq!(f.build.system, "autotools");

        let b = f.bottle.unwrap();
        assert_eq!(b.arch, "x86_64");
        assert_eq!(b.tags.len(), 2);
        let monterey = b.tags.get("monterey").unwrap();
        assert_eq!(monterey.sha256, "aaa");
        assert_eq!(
            monterey.url,
            "https://example.com/bottles/hello/2.12.1_1/hello-2.12.1_1.monterey.bottle.tar.gz"
        );
        assert!(!b.tags.contains_key("arm64_monterey"));

        let s = f.service.unwrap();
        assert_eq!(s.run, "opt/hello/bin/hello --flag");
        assert!(s.keep_alive);
        assert_eq!(s.log_path, "var/log/hello.log");
    }

    #[test]
    fn test_convert_cask() {
        let src = r#"
cask "demo" do
  version "1.2.3"
  sha256 "abc123"
  name "Demo App"
  desc "Demo application"
  homepage "https://example.com/"
  url "https://example.com/demo-1.2.3.zip"
  app "Demo.app"
  binary "demo", target: "demo-cli"

  livecheck do
    url "https://example.com/latest"
  end
end
"#;
        let c = convert_cask(src, "demo").unwrap();
        assert_eq!(c.kind, Kind::Cask);
        let k = Cask::parse(&c.toml).unwrap();
        assert_eq!(k.package.token, "demo");
        assert_eq!(k.package.name, "Demo App");
        assert_eq!(k.package.version, "1.2.3");
        assert_eq!(k.source.url, "https://example.com/demo-1.2.3.zip");
        assert_eq!(k.artifacts.len(), 2);
        assert_eq!(k.artifacts[0].kind, "app");
        assert_eq!(k.artifacts[0].path, "Demo.app");
        assert_eq!(k.artifacts[1].kind, "binary");
        assert_eq!(k.artifacts[1].target, "demo-cli");
    }
}
