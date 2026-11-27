use anyhow::Result;
use std::path::PathBuf;

/// Tap 的角色类型，影响 formula 搜索优先级与 `fb tap list` 显示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapRole {
    /// 普通第三方 tap（默认）
    User,
    /// 官方源码构建核心 tap：formula 只有 [source]，无 [bottle]
    SeedsCore,
    /// 官方 bottle 优先核心 tap：formula 同时有 [source] 和 [bottle]
    FieldsCore,
}

impl TapRole {
    /// 优先级（数值越大越优先选同名 formula）：fields-core > seeds-core > 普通 tap
    pub fn priority(&self) -> u8 {
        match self {
            TapRole::FieldsCore => 3,
            TapRole::SeedsCore => 2,
            TapRole::User => 1,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            TapRole::User => "user",
            TapRole::SeedsCore => "core·source",
            TapRole::FieldsCore => "core·bottle",
        }
    }
}

/// 内置核心 tap 定义
pub struct BuiltinTap {
    pub name: &'static str,
    pub repo: &'static str,
    pub role: TapRole,
    pub description: &'static str,
    pub github_org: &'static str,
}

pub const SEEDS_CORE: BuiltinTap = BuiltinTap {
    name: "fieldsbrew/seeds-core",
    repo: "seeds-core",
    role: TapRole::SeedsCore,
    description: "官方源码构建核心 tap —— formula 只有 [source]，无 [bottle]",
    github_org: "yhcmac-prog",
};

pub const FIELDS_CORE: BuiltinTap = BuiltinTap {
    name: "fieldsbrew/fields-core",
    repo: "fields-core",
    role: TapRole::FieldsCore,
    description: "官方 bottle 优先核心 tap —— formula 同时有 [source] 和 [bottle]",
    github_org: "yhcmac-prog",
};

#[allow(dead_code)]
pub const ALL_BUILTIN_TAPS: &[BuiltinTap] = &[SEEDS_CORE, FIELDS_CORE];

/// tap 元信息（解析自 tap 目录下的 .fieldsbrew.toml 或根据目录名推断）
#[derive(Debug, Clone)]
pub struct TapInfo {
    pub user: String,
    pub repo: String,
    pub path: PathBuf,
    pub role: TapRole,
}

impl TapInfo {
    pub fn short_name(&self) -> String {
        format!("{}/{}", self.user, self.repo.strip_prefix("homebrew-").unwrap_or(&self.repo))
    }
}

/// 全局配置与路径布局。
///
/// 目录结构（prefix 默认 /opt/fieldsbrew，可用 FIELDSBREW_PREFIX 覆盖）：
/// ```text
/// <prefix>/
/// ├── Cellar/<name>/<version>/   # 每个版本独立目录
/// ├── opt/<name>                 # 指向当前版本的符号链接（keg-only 除外）
/// ├── Formula/*.toml             # 本地 formula 目录
/// ├── var/db/fieldsbrew/installed/<name>.json   # 安装数据库（manifest）
/// └── tmp/                       # 构建临时目录
/// ```
#[derive(Debug, Clone)]
pub struct Config {
    pub prefix: PathBuf,
    pub verbose: bool,
}

impl Config {
    pub fn new(verbose: bool) -> Self {
        let prefix = std::env::var("FIELDSBREW_PREFIX")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/opt/fieldsbrew"));
        Config { prefix, verbose }
    }

    pub fn cellar(&self) -> PathBuf {
        self.prefix.join("Cellar")
    }

    pub fn opt(&self) -> PathBuf {
        self.prefix.join("opt")
    }

    pub fn db_dir(&self) -> PathBuf {
        self.prefix.join("var/db/fieldsbrew/installed")
    }

    /// Cask 安装记录目录
    pub fn cask_db_dir(&self) -> PathBuf {
        self.prefix.join("var/db/fieldsbrew/casks")
    }

    pub fn tmp(&self) -> PathBuf {
        self.prefix.join("tmp")
    }

    /// 第三方 tap 根目录（`<prefix>/Library/Taps`）
    pub fn taps_root(&self) -> PathBuf {
        self.prefix.join("Library/Taps")
    }

    /// 已添加的 tap 目录（`<taps_root>/<user>/<repo>`）
    pub fn tap_dirs(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let root = self.taps_root();
        let Ok(users) = std::fs::read_dir(&root) else {
            return out;
        };
        for user in users.flatten() {
            if !user.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            if let Ok(repos) = std::fs::read_dir(user.path()) {
                for repo in repos.flatten() {
                    if repo.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        out.push(repo.path());
                    }
                }
            }
        }
        out
    }

    /// 识别 tap 角色：内置核心 tap 按目录名匹配；其他读 tap 目录下的
    /// `.fieldsbrew.toml` 里的 `role = "seeds" | "fields"`，否则默认 User。
    pub fn tap_info(&self, path: &PathBuf) -> TapInfo {
        let user = path
            .parent()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let repo = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        let role = if repo == SEEDS_CORE.repo {
            TapRole::SeedsCore
        } else if repo == FIELDS_CORE.repo {
            TapRole::FieldsCore
        } else {
            self.detect_tap_role_from_dir(path)
        };

        TapInfo {
            user,
            repo,
            path: path.clone(),
            role,
        }
    }

    fn detect_tap_role_from_dir(&self, dir: &PathBuf) -> TapRole {
        let meta = dir.join(".fieldsbrew.toml");
        if let Ok(text) = std::fs::read_to_string(&meta) {
            if let Ok(val) = toml::from_str::<toml::Value>(&text) {
                if let Some(role) = val.get("tap").and_then(|t| t.get("role")).and_then(|r| r.as_str()) {
                    return match role {
                        "seeds" | "seeds-core" | "source" => TapRole::SeedsCore,
                        "fields" | "fields-core" | "bottle" => TapRole::FieldsCore,
                        _ => TapRole::User,
                    };
                }
            }
        }
        TapRole::User
    }

    /// 按优先级排序的 tap 元信息（fields-core > seeds-core > 其他 tap）。
    /// 同名 tap 只保留第一个（避免重复搜索）。
    pub fn taps_sorted(&self) -> Vec<TapInfo> {
        let mut infos: Vec<TapInfo> = self.tap_dirs().iter().map(|p| self.tap_info(p)).collect();
        infos.sort_by(|a, b| {
            let c = b.role.priority().cmp(&a.role.priority());
            if c != std::cmp::Ordering::Equal {
                return c;
            }
            a.short_name().cmp(&b.short_name())
        });
        infos
    }

    /// 检查某个内置核心 tap 是否已添加（目录存在即视为已添加，不校验 git 状态）
    pub fn has_builtin_tap(&self, builtin: &BuiltinTap) -> bool {
        let dir = self.taps_root().join("fieldsbrew").join(builtin.repo);
        dir.exists()
    }

    /// 核心 tap 的 tap 根目录（用户级 tap 放这里，类似 homebrew/core 的位置）
    pub fn core_taps_root(&self) -> PathBuf {
        self.taps_root().join("fieldsbrew")
    }

    /// tap 内可能的 formula 目录：Formula/、HomebrewFormula/ 与 tap 根目录。
    /// 按 tap 优先级排序（fields-core > seeds-core > 其他 tap），
    /// 同名 formula 出现在多个 tap 时，排在前面的会被优先使用。
    pub fn tap_formula_dirs(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for info in self.taps_sorted() {
            for sub in ["Formula", "HomebrewFormula"] {
                let d = info.path.join(sub);
                if d.exists() {
                    out.push(d);
                }
            }
            out.push(info.path.clone());
        }
        out
    }

    /// formula 搜索目录（先环境变量覆盖，后 prefix 内置，最后 taps——taps 按优先级排序）
    pub fn formula_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Ok(d) = std::env::var("FIELDSBREW_FORMULA_DIR") {
            dirs.push(PathBuf::from(d));
        }
        dirs.push(self.prefix.join("Formula"));
        dirs.extend(self.tap_formula_dirs());
        dirs
    }

    /// cask 搜索目录（同样按 tap 优先级排序）
    pub fn cask_dirs(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Ok(d) = std::env::var("FIELDSBREW_CASK_DIR") {
            out.push(PathBuf::from(d));
        }
        out.push(self.prefix.join("Casks"));
        for info in self.taps_sorted() {
            for sub in ["Casks", "HomebrewCask"] {
                let d = info.path.join(sub);
                if d.exists() {
                    out.push(d);
                }
            }
        }
        out
    }

    /// 创建所有必需目录；失败时给出可操作的提示
    pub fn ensure_dirs(&self) -> Result<()> {
        for d in [self.cellar(), self.opt(), self.db_dir(), self.tmp()] {
            std::fs::create_dir_all(&d).map_err(|e| {
                anyhow::anyhow!(
                    "无法创建目录 {}（{e}）\n提示: 默认 prefix 为 /opt/fieldsbrew，需要管理员权限:\n  \
                     sudo mkdir -p /opt/fieldsbrew && sudo chown -R $(whoami) /opt/fieldsbrew\n  \
                     开发/测试时可改用环境变量 FIELDSBREW_PREFIX 指定可写目录",
                    d.display()
                )
            })?;
        }
        Ok(())
    }
}
