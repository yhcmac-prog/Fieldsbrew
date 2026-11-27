#!/usr/bin/env python3
"""批量生成 Fieldsbrew Formula TOML 文件。

对每个包：
  1. 查最新版本（从发布源 API / 目录页面 / GitHub Releases）
  2. 拼源码 tarball URL
  3. 下载算 sha256
  4. 输出到 Formula/<name>.toml

用法: python3 gen_formulas.py [--dry-run] [--out-dir Formula]
"""

import argparse
import json
import re
import subprocess
import sys
import urllib.request
import urllib.error
from pathlib import Path

def curl(url: str, ipv6: bool = True, timeout: int = 15) -> bytes:
    """用 curl 下载，优先 IPv6。"""
    proto = "-6" if ipv6 else "-4"
    r = subprocess.run(
        ["curl", "-sS", proto, "-m", str(timeout), "-L", url, "-o", "-"],
        capture_output=True, timeout=timeout + 5,
    )
    if r.returncode != 0:
        raise RuntimeError(f"curl {url} failed: {r.stderr.decode(errors='ignore')}")
    return r.stdout

def curl_text(url: str, ipv6: bool = True) -> str:
    return curl(url, ipv6).decode(errors="replace")

def curl_headers(url: str, ipv6: bool = True, timeout: int = 10) -> dict:
    proto = "-6" if ipv6 else "-4"
    r = subprocess.run(
        ["curl", "-sSI", proto, "-m", str(timeout), url],
        capture_output=True, timeout=timeout + 5,
    )
    out = r.stdout.decode(errors="replace").splitlines()
    return {l.split(":", 1)[0].lower(): l.split(":", 1)[1].strip() for l in out[1:] if ":" in l}

def sha256_file(path: Path) -> str:
    import hashlib
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()

def download_to(url: str, dest: Path, ipv6: bool = True, max_mb: int = 100) -> None:
    """下载到文件。超过 max_mb 时跳过，文件不存在则写入空文件占位。"""
    proto = "-6" if ipv6 else "-4"
    dest.parent.mkdir(parents=True, exist_ok=True)
    # 先查大小
    try:
        headers = curl_headers(url, ipv6=ipv6)
        size_mb = int(headers.get("content-length", "0")) / (1024 * 1024)
        if size_mb > max_mb:
            print(f"  ⏭ 跳过下载（{size_mb:.0f} MB > {max_mb} MB 上限）")
            return
    except Exception:
        pass  # 不确定就试着下

    r = subprocess.run(
        ["curl", "-sSL", proto, "-C", "-", "-m", "600", "-o", str(dest), url],
        capture_output=True, timeout=620,
    )
    if r.returncode != 0 or not dest.exists() or dest.stat().st_size == 0:
        raise RuntimeError(f"下载 {url} 失败: {r.stderr.decode(errors='ignore')}")

# --- 各包版本获取策略 ---

def latest_python() -> tuple[str, str]:
    """python.org FTP 目录页找最新稳定版。"""
    html = curl_text("https://www.python.org/ftp/python/", ipv6=True)
    # 3.x.y 格式，排除 alpha/beta/rc
    versions = re.findall(r'href="(3\.\d+\.\d+)/"', html)
    # 过滤掉 alpha/beta/rc：目录名里没有 a/b/rc，但里面可能有
    # 进一步检查目录内容是否有 Python-<ver>.tgz（稳定版）
    stable = []
    for v in versions:
        if any(tag in v for tag in ["a", "b", "rc"]):
            continue
        # 检查目录里是否有 Python-<ver>.tgz（稳定版标记）
        try:
            d = curl_text(f"https://www.python.org/ftp/python/{v}/", ipv6=True)
            if f"Python-{v}.tgz" in d:
                stable.append(v)
        except Exception:
            continue
    if not stable:
        raise RuntimeError("Python 版本解析失败")
    def key(v): return tuple(int(x) for x in v.split("."))
    ver = sorted(stable, key=key)[-1]
    url = f"https://www.python.org/ftp/python/{ver}/Python-{ver}.tgz"
    return ver, url

def latest_node() -> tuple[str, str]:
    """nodejs.org/dist/index.json。"""
    text = curl_text("https://nodejs.org/dist/index.json", ipv6=True)
    items = json.loads(text)
    # LTS 优先（version 形如 v20.11.0）
    lts = [x for x in items if x.get("lts")]
    if lts:
        item = lts[0]
    else:
        item = items[0]
    ver = item["version"].lstrip("v")
    url = f"https://nodejs.org/dist/v{ver}/node-v{ver}.tar.xz"
    return ver, url

def latest_gcc() -> tuple[str, str]:
    """gnu.org 目录页找最高 gcc-* 版本。"""
    html = curl_text("https://ftp.gnu.org/gnu/gcc/", ipv6=True)
    versions = re.findall(r'gcc-(\d+\.\d+\.\d+)\.tar\.xz', html)
    if not versions:
        versions = re.findall(r'gcc-(\d+\.\d+\.\d+)\/', html)
    stable = [v for v in versions if not any(tag in v for tag in ["a", "b", "rc"])]
    if not stable:
        raise RuntimeError("GCC 版本解析失败")
    def key(v): return tuple(int(x) for x in v.split("."))
    ver = sorted(set(stable), key=key)[-1]
    url = f"https://ftp.gnu.org/gnu/gcc/gcc-{ver}/gcc-{ver}.tar.xz"
    return ver, url

def latest_llvm_clang() -> tuple[str, str]:
    """GitHub API 查 llvm-project release。"""
    text = curl_text("https://api.github.com/repos/llvm/llvm-project/releases/latest", ipv6=False)
    data = json.loads(text)
    tag = data["tag_name"]  # llvmorg-18.1.8
    ver = re.sub(r"^llvmorg-", "", tag)
    url = f"https://github.com/llvm/llvm-project/archive/refs/tags/{tag}.tar.gz"
    return ver, url

def latest_glib() -> tuple[str, str]:
    """gnome.org 目录页。"""
    html = curl_text("https://download.gnome.org/sources/glib/", ipv6=True)
    versions = re.findall(r'href="(\d+\.\d+)/"', html)
    if not versions:
        raise RuntimeError("glib 版本解析失败")
    major = sorted(versions)[-1]  # 2.80
    html2 = curl_text(f"https://download.gnome.org/sources/glib/{major}/", ipv6=True)
    files = re.findall(r'glib-(\d+\.\d+\.\d+)\.tar\.xz', html2)
    if not files:
        files = re.findall(r'glib-(\d+\.\d+\.\d+)\.tar\.gz', html2)
    if not files:
        raise RuntimeError("glib 子版本解析失败")
    def key(v): return tuple(int(x) for x in v.split("."))
    ver = sorted(set(files), key=key)[-1]
    url = f"https://download.gnome.org/sources/glib/{major}/glib-{ver}.tar.xz"
    return ver, url

def latest_qemu() -> tuple[str, str]:
    """qemu.org 目录页。"""
    html = curl_text("https://download.qemu.org/", ipv6=True)
    versions = re.findall(r'qemu-(\d+\.\d+\.\d+)\.tar\.xz', html)
    if not versions:
        versions = re.findall(r'qemu-(\d+\.\d+\.\d+)\.tar\.gz', html)
    stable = [v for v in versions if not any(tag in v for tag in ["rc", "alpha", "beta"])]
    if not stable:
        raise RuntimeError("QEMU 版本解析失败")
    def key(v): return tuple(int(x) for x in v.split("."))
    ver = sorted(set(stable), key=key)[-1]
    url = f"https://download.qemu.org/qemu-{ver}.tar.xz"
    return ver, url

def latest_ninja() -> tuple[str, str]:
    """GitHub API。"""
    text = curl_text("https://api.github.com/repos/ninja-build/ninja/releases/latest", ipv6=False)
    data = json.loads(text)
    ver = data["tag_name"].lstrip("v")
    # 源码 tarball
    url = f"https://github.com/ninja-build/ninja/archive/refs/tags/v{ver}.tar.gz"
    return ver, url

def latest_ghc() -> tuple[str, str]:
    """haskell.org/ghc 页面或 tarball 列表。"""
    html = curl_text("https://www.haskell.org/ghc/download.html", ipv6=True)
    versions = re.findall(r'ghc-(\d+\.\d+\.\d+)-src\.tar\.xz', html)
    if not versions:
        # 备选：直接查 haskell.org
        text2 = curl_text("https://downloads.haskell.org/~ghc/latest/", ipv6=True)
        versions = re.findall(r'ghc-(\d+\.\d+\.\d+)', text2)
    stable = [v for v in versions if not any(tag in v for tag in ["a", "b", "rc"])]
    if not stable:
        raise RuntimeError("GHC 版本解析失败")
    def key(v): return tuple(int(x) for x in v.split("."))
    ver = sorted(set(stable), key=key)[-1]
    url = f"https://downloads.haskell.org/~ghc/{ver}/ghc-{ver}-src.tar.xz"
    return ver, url

def latest_ollama() -> tuple[str, str]:
    """GitHub API ollama/ollama。"""
    text = curl_text("https://api.github.com/repos/ollama/ollama/releases/latest", ipv6=False)
    data = json.loads(text)
    ver = data["tag_name"].lstrip("v")
    url = f"https://github.com/ollama/ollama/archive/refs/tags/v{ver}.tar.gz"
    return ver, url

def latest_thrive() -> tuple[str, str]:
    """GitHub API Revolutionary-Games/Thrive。"""
    text = curl_text("https://api.github.com/repos/Revolutionary-Games/Thrive/releases/latest", ipv6=False)
    data = json.loads(text)
    ver = data["tag_name"]
    url = f"https://github.com/Revolutionary-Games/Thrive/archive/refs/tags/{ver}.tar.gz"
    return ver, url

def latest_wine_channel(channel: str) -> tuple[str, str, str]:
    """Wine 三个 channel: stable / devel / staging。"""
    html = curl_text("https://www.winehq.org/download", ipv6=True)
    # 找 stable: wine-8.0 / devel: wine-9.x / staging: wine-9.x
    if channel == "stable":
        m = re.search(r'wine-(8\.\d+)\.tar\.xz', html)
        if not m:
            # 备选：直接用已知版本
            return "8.21", "https://dl.winehq.org/wine/source/8.0/wine-8.21.tar.xz", "stable"
        ver = m.group(1)
        url = f"https://dl.winehq.org/wine/source/8.0/wine-{ver}.tar.xz"
    elif channel == "devel":
        m = re.search(r'wine-(9\.\d+\.\d+)\.tar\.xz', html)
        if not m:
            return "9.20", "https://dl.winehq.org/wine/source/9.x/wine-9.20.tar.xz", "devel"
        ver = m.group(1)
        url = f"https://dl.winehq.org/wine/source/9.x/wine-{ver}.tar.xz"
    elif channel == "staging":
        # staging 版本号基本跟 devel 一样，只是 git apply 额外 patch
        ver, _url, _ = latest_wine_channel("devel")
        url = f"https://dl.winehq.org/wine/source/9.x/wine-{ver}.tar.xz"
        return ver, url, "staging"
    else:
        raise ValueError(channel)
    return ver, url, channel

# --- 包定义 ---

PACKAGES = [
    {
        "name": "python", "short": "python3",
        "description": "Python 编程语言（官方 CPython）",
        "homepage": "https://www.python.org/",
        "fetch": latest_python,
        "build_system": "autotools",
        "deps": ["openssl@1.1", "sqlite", "zlib", "bzip2", "xz"],
        "keg_only": False,
    },
    {
        "name": "node", "short": "node",
        "description": "Node.js JavaScript 运行时（LTS）",
        "homepage": "https://nodejs.org/",
        "fetch": latest_node,
        "build_system": "make",
        "deps": [],
        "keg_only": False,
    },
    {
        "name": "gcc", "short": "gcc",
        "description": "GNU Compiler Collection（C/C++/Fortran/Go/ObjC）",
        "homepage": "https://gcc.gnu.org/",
        "fetch": latest_gcc,
        "build_system": "autotools",
        "deps": ["gmp", "mpfr", "mpc", "isl", "zlib"],
        "keg_only": True,
    },
    {
        "name": "clang", "short": "clang",
        "description": "LLVM Clang C/C++/ObjC 编译器",
        "homepage": "https://clang.llvm.org/",
        "fetch": latest_llvm_clang,
        "build_system": "cmake",
        "deps": ["libffi", "zlib"],
        "keg_only": True,
    },
    {
        "name": "glib", "short": "glib",
        "description": "GNOME GLib 核心工具库",
        "homepage": "https://wiki.gnome.org/Projects/GLib",
        "fetch": latest_glib,
        "build_system": "cmake",
        "deps": ["gettext", "libffi", "pcre2"],
        "keg_only": False,
    },
    {
        "name": "qemu", "short": "qemu",
        "description": "QEMU 通用机器模拟器与虚拟化",
        "homepage": "https://www.qemu.org/",
        "fetch": latest_qemu,
        "build_system": "meson",
        "deps": ["glib", "pixman", "slirp"],
        "keg_only": False,
    },
    {
        "name": "ninja", "short": "ninja",
        "description": "Ninja 快速构建系统",
        "homepage": "https://ninja-build.org/",
        "fetch": latest_ninja,
        "build_system": "cmake",
        "deps": [],
        "keg_only": False,
    },
    {
        "name": "ghc", "short": "ghc",
        "description": "Glasgow Haskell Compiler（Haskell 编译器与运行时）",
        "homepage": "https://www.haskell.org/ghc/",
        "fetch": latest_ghc,
        "build_system": "make",
        "deps": [],
        "keg_only": True,
    },
    {
        "name": "ollama", "short": "ollama",
        "description": "Ollama — 本地运行大型语言模型",
        "homepage": "https://ollama.com/",
        "fetch": latest_ollama,
        "build_system": "make",
        "deps": [],
        "keg_only": True,
    },
    {
        "name": "thrive", "short": "thrive",
        "description": "Thrive — 开源进化策略游戏（Revolutionary Games）",
        "homepage": "https://revolutionarygamesstudio.com/",
        "fetch": latest_thrive,
        "build_system": "cmake",
        "deps": [],
        "keg_only": True,
    },
    {
        "name": "wine-stable", "short": "wine",
        "description": "Wine stable — 让 Intel Mac 运行 Windows 程序（稳定版）",
        "homepage": "https://www.winehq.org/",
        "fetch": lambda: latest_wine_channel("stable"),
        "fetch_many": True,
        "build_system": "autotools",
        "deps": [],
        "keg_only": False,
    },
    {
        "name": "wine-devel", "short": "wine",
        "description": "Wine devel — 开发/测试版",
        "homepage": "https://www.winehq.org/",
        "fetch": lambda: latest_wine_channel("devel"),
        "fetch_many": True,
        "build_system": "autotools",
        "deps": [],
        "keg_only": False,
    },
    {
        "name": "wine-staging", "short": "wine",
        "description": "Wine staging — 开发版 + 额外 patch",
        "homepage": "https://www.winehq.org/",
        "fetch": lambda: latest_wine_channel("staging"),
        "fetch_many": True,
        "build_system": "autotools",
        "deps": [],
        "keg_only": False,
    },
]


def generate_toml(pkg: dict, version: str, url: str, sha256: str, channel: str | None = None) -> str:
    lines = []
    lines.append(f"[package]")
    lines.append(f'name = "{pkg["name"]}"')
    lines.append(f'version = "{version}"')
    lines.append(f'description = "{pkg["description"]}"')
    lines.append(f'homepage = "{pkg["homepage"]}"')
    if pkg.get("deps"):
        deps = ", ".join(f'"{d}"' for d in pkg["deps"])
        lines.append(f"dependencies = [{deps}]")
    lines.append("")
    lines.append("[source]")
    lines.append(f'url = "{url}"')
    lines.append(f'sha256 = "{sha256}"')
    lines.append("")
    lines.append("[build]")
    lines.append(f'system = "{pkg["build_system"]}"')
    lines.append("")
    lines.append("[install]")
    lines.append(f'keg_only = {"true" if pkg["keg_only"] else "false"}')
    lines.append("")
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--dry-run", action="store_true", help="只查版本/URL，不下载/算 hash")
    parser.add_argument("--out-dir", default="Formula")
    parser.add_argument("--only", help="只处理指定包名（逗号分隔）")
    args = parser.parse_args()

    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    only = set(args.only.split(",")) if args.only else None

    for pkg in PACKAGES:
        if only and pkg["name"] not in only:
            continue
        name = pkg["name"]
        print(f"\n=== {name} ===")
        sys.stdout.flush()

        try:
            result = pkg["fetch"]()
            if pkg.get("fetch_many"):
                version, url, _channel = result
            else:
                # 有些 fetch 返回 3 个值（channel）
                if isinstance(result, tuple) and len(result) == 3:
                    version, url, _ = result
                else:
                    version, url = result
            print(f"  版本: {version}")
            print(f"  URL : {url}")
        except Exception as e:
            print(f"  ❌ 获取版本失败: {e}")
            continue

        if args.dry_run:
            continue

        # 下载 + 算 sha256
        cache_dir = Path("/tmp/fb-gen-cache")
        cache_dir.mkdir(exist_ok=True)
        fname = url.rsplit("/", 1)[-1]
        fpath = cache_dir / fname

        sha256 = ""
        try:
            if not fpath.exists() or fpath.stat().st_size == 0:
                print(f"  下载中...", end=" ", flush=True)
                download_to(url, fpath, ipv6=True)
                print(f"OK ({fpath.stat().st_size//1024} KB)")
            sha256 = sha256_file(fpath)
            print(f"  sha256: {sha256}")
        except Exception as e:
            print(f"  ❌ 下载/校验失败: {e}")
            sha256 = ""  # Fieldsbrew 允许空 sha256，警告但不强制

        # 生成 TOML
        toml = generate_toml(pkg, version, url, sha256)
        out = out_dir / f"{name}.toml"
        out.write_text(toml, encoding="utf-8")
        print(f"  ✅ 写入 {out}")


if __name__ == "__main__":
    main()
