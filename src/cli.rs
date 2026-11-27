use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum ServicesCommand {
    /// 运行服务但不注册登录自启
    Run { name: String },
    /// 注册服务并立即启动（随登录自启）
    Start { name: String },
    /// 停止服务
    Stop { name: String },
    /// 重启服务
    Restart { name: String },
    /// 列出服务状态
    List,
    /// 清理已卸载或已失效的服务配置
    Cleanup,
}

#[derive(Subcommand)]
pub enum CaskCommand {
    /// 安装 cask（.app / 可执行文件 / 字体）
    Install {
        /// cask token，可一次多个
        tokens: Vec<String>,
        /// 覆盖已存在的目标文件
        #[arg(long)]
        force: bool,
    },
    /// 卸载 cask
    Uninstall { tokens: Vec<String> },
    /// 列出已安装的 cask
    List,
    /// 搜索 cask（按 token/名称/描述匹配）
    Search {
        /// 关键字；省略则列出全部
        query: Option<String>,
    },
}

#[derive(Parser)]
#[command(
    name = "fb",
    version,
    about = "Fieldsbrew — 功能与 Homebrew 对齐的 Intel Mac 包管理器",
    long_about = "兼容 Homebrew 的 Cellar 布局、链接语义与 bottle 校验方式，面向 Intel Mac。\n源码编译与预编译 bottle 双路径：bottle 不可用时自动回退源码构建。"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// 输出详细日志（显示执行的命令）
    #[arg(short = 'v', long, global = true)]
    pub verbose: bool,
}

#[derive(Subcommand)]
pub enum Command {
    /// 安装 formula（自动递归安装依赖）
    Install {
        /// 包名，可一次多个
        names: Vec<String>,
        /// 跳过 sandbox-exec 禁网沙盒
        #[arg(long)]
        no_sandbox: bool,
        /// 保留临时构建目录以便调试
        #[arg(long)]
        keep_tmp: bool,
    },
    /// 卸载已安装的包
    Uninstall {
        names: Vec<String>,
        /// 即使有其他包依赖它也强制卸载
        #[arg(long)]
        force: bool,
    },
    /// 升级已安装的包；不指定包名则升级全部过期包
    Upgrade {
        /// 包名；省略则升级所有可升级的包
        names: Vec<String>,
        /// 跳过 sandbox-exec 禁网沙盒
        #[arg(long)]
        no_sandbox: bool,
        /// 保留临时构建目录以便调试
        #[arg(long)]
        keep_tmp: bool,
    },
    /// 重装已安装的包（先移除再重新构建）
    Reinstall {
        names: Vec<String>,
        /// 跳过 sandbox-exec 禁网沙盒
        #[arg(long)]
        no_sandbox: bool,
        /// 保留临时构建目录
        #[arg(long)]
        keep_tmp: bool,
    },
    /// 列出可升级的包
    Outdated,
    /// 更新配方仓库（对已添加的 tap 执行 git pull）
    Update,
    /// 搜索 formula（按名称或描述匹配）
    Search {
        /// 关键字；省略则列出全部
        query: Option<String>,
    },
    /// 查看 formula 详情
    Info { name: String },
    /// 列出已安装的包
    List {
        /// 只读显示 Homebrew Cellar 中的已安装包
        #[arg(long)]
        brew: bool,
    },
    /// 查看依赖
    Deps {
        name: String,
        /// 以树形展示
        #[arg(long)]
        tree: bool,
    },
    /// 查看反向依赖（哪些 formula 依赖它）
    Uses { name: String },
    /// 手动把已安装包链接到 prefix（keg-only 需要 --force）
    Link {
        names: Vec<String>,
        /// 允许链接 keg-only 包，并替换同名普通文件
        #[arg(long)]
        force: bool,
    },
    /// 取消已安装包在 prefix 下的链接
    Unlink { names: Vec<String> },
    /// 列出、添加或移除第三方配方仓库（tap）
    Tap {
        /// user/repo 或 git URL；省略则列出已添加的 tap
        name: Option<String>,
        /// 自定义 git URL（配合 name 指定本地 tap 目录名）
        #[arg(long)]
        url: Option<String>,
        /// 移除指定 tap
        #[arg(long)]
        remove: bool,
        /// 一键添加官方核心 tap（seeds-core 源码构建 + fields-core bottle 优先）
        #[arg(long)]
        core: bool,
    },
    /// 输出 shell 环境变量设置（可 eval 使用）
    Shellenv {
        /// 指定 shell：sh/bash/zsh、fish、csh/tcsh；默认按 $SHELL 自动识别
        #[arg(long)]
        shell: Option<String>,
    },
    /// 将 Homebrew Ruby formula/cask 转换为 Fieldsbrew TOML
    Convert {
        /// .rb 文件，或包含 .rb 的目录
        path: PathBuf,
        /// 输出目录；单文件省略时输出到 stdout
        #[arg(long)]
        out: Option<PathBuf>,
        /// 强制按 Cask 解析
        #[arg(long)]
        cask: bool,
    },
    /// 管理 Cask（GUI 应用/字体/二进制）
    Cask {
        #[command(subcommand)]
        command: CaskCommand,
    },
    /// 为发布产物生成 Ed25519 分离签名（生成 <file>.sig 与 <file>.pub）
    Sign {
        /// 要签名的文件
        file: PathBuf,
        /// 32 字节 seed 的 hex 文件；省略时读取 FIELDSBREW_SIGNING_KEY
        #[arg(long)]
        key_file: Option<PathBuf>,
        /// 签名输出路径（默认 <file>.sig）
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// 校验发布产物的 Ed25519 分离签名
    Verify {
        /// 被签名文件
        file: PathBuf,
        /// 签名文件（默认 <file>.sig）
        #[arg(long)]
        signature: Option<PathBuf>,
        /// 公钥 hex 或文件（默认 <file>.pub）
        #[arg(long)]
        public_key: Option<String>,
    },
    /// 诊断网络连通性（DNS / IPv4 / IPv6 / 代理 / 镜像 / GitHub 端点）
    Net {
        /// 单个端点 TCP 探测超时（秒）
        #[arg(long, default_value_t = 4)]
        timeout: u64,
    },
    /// 自更新 fb 二进制（GitHub Releases，支持镜像、代理与签名校验）
    SelfUpdate {
        /// 仅检查是否有新版本，不下载
        #[arg(long)]
        check: bool,
        /// 直接指定发布包 URL（跳过 GitHub API）
        #[arg(long)]
        url: Option<String>,
        /// GitHub 仓库 owner/repo；默认读 FIELDSBREW_UPDATE_REPO 或 fieldsbrew/fieldsbrew
        #[arg(long)]
        repo: Option<String>,
        /// 目标版本标签（如 v0.2.0）
        #[arg(long)]
        version: Option<String>,
        /// 版本相同时也强制替换
        #[arg(long)]
        force: bool,
        /// 显式校验 sha256（省略时尝试 Release 中的 sha256sums.txt）
        #[arg(long)]
        sha256: Option<String>,
        /// 校验发布包签名所用的公钥（hex 或文件路径）
        #[arg(long)]
        public_key: Option<String>,
    },
    /// 管理后台服务（launchd）
    Services {
        #[command(subcommand)]
        command: Option<ServicesCommand>,
    },
    /// 从 Homebrew 迁移包到 Fieldsbrew（不自动卸载 Homebrew 侧）
    Migrate {
        /// 包名；省略则列出可迁移的 Homebrew 包
        names: Vec<String>,
        /// 跳过 sandbox-exec 禁网沙盒
        #[arg(long)]
        no_sandbox: bool,
        /// 保留临时构建目录
        #[arg(long)]
        keep_tmp: bool,
    },
    /// 审计 formula（字段、URL、校验和、依赖、构建系统）
    Audit {
        /// 指定 formula；省略则审计所有 formula
        names: Vec<String>,
    },
    /// 把已构建的 Cellar 目录打包成 bottle tarball（Homebrew 兼容格式）
    Bottle {
        /// formula 名，可一次多个；需已安装或自动构建
        names: Vec<String>,
        /// 输出目录（默认 ./bottles）
        #[arg(long)]
        out: Option<PathBuf>,
        /// 跳过自动构建（仅打包已安装版本）
        #[arg(long)]
        skip_build: bool,
        /// 构建时保留临时目录
        #[arg(long)]
        keep_tmp: bool,
        /// 构建时跳过 sandbox
        #[arg(long)]
        no_sandbox: bool,
    },
    /// 清理旧版本、悬空链接与构建临时目录
    Cleanup,
    /// 检查本机环境是否满足安装条件
    Doctor,
}
