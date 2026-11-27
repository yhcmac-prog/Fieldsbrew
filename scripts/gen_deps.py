#!/usr/bin/env python3
"""批量生成依赖子 formula —— 补全 audit 中提示缺失的依赖。

用法: python3 scripts/gen_deps.py
"""

import hashlib
import subprocess
import sys
from pathlib import Path


def curl(cmd: list[str]) -> bytes:
    r = subprocess.run(["curl", "-sSL", "-m", "600", "-o", "-", *cmd],
                       capture_output=True, timeout=620)
    if r.returncode != 0:
        raise RuntimeError(r.stderr.decode(errors="replace"))
    return r.stdout


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def download(url: str, dest: Path) -> str:
    dest.parent.mkdir(parents=True, exist_ok=True)
    # 用 -C - 断点续传
    r = subprocess.run(["curl", "-sSL", "-C", "-", "-m", "600", "-o", str(dest), url],
                       capture_output=True, timeout=620)
    if r.returncode != 0:
        raise RuntimeError(f"下载 {url} 失败: {r.stderr.decode(errors='replace')}")
    return sha256_file(dest)


# 依赖包的硬编码版本与 URL（Homebrew/MacPorts 常用的稳定版本）
# 优先 IPv6 可达的源
DEPS = [
    # --- python 依赖 ---
    {
        "name": "openssl@1.1", "version": "1.1.1w",
        "url": "https://www.openssl.org/source/old/1.1.1/openssl-1.1.1w.tar.gz",
        "sha256": "",  # 会自动下载填
        "build": "make", "keg_only": True,
        "desc": "OpenSSL 1.1.1 — TLS/SSL 加密库（长期支持）",
        "homepage": "https://www.openssl.org/",
        "deps": [],
    },
    {
        "name": "sqlite", "version": "3.47.0",
        "url": "https://www.sqlite.org/2024/sqlite-autoconf-3470000.tar.gz",
        "sha256": "",
        "build": "autotools", "keg_only": False,
        "desc": "SQLite — 单文件嵌入式 SQL 数据库引擎",
        "homepage": "https://www.sqlite.org/",
        "deps": [],
    },
    {
        "name": "bzip2", "version": "1.0.8",
        "url": "https://www.sourceware.org/pub/bzip2/bzip2-1.0.8.tar.gz",
        "sha256": "",
        "build": "make", "keg_only": False,
        "desc": "bzip2 数据压缩库",
        "homepage": "https://www.sourceware.org/bzip2/",
        "deps": [],
    },
    {
        "name": "xz", "version": "5.8",
        "url": "https://tukaani.org/xz/xz-5.8.tar.xz",
        "sha256": "",
        "build": "autotools", "keg_only": False,
        "desc": "XZ 压缩格式实现（LZMA/LZMA2）",
        "homepage": "https://tukaani.org/xz/",
        "deps": [],
    },

    # --- gcc 依赖 ---
    {
        "name": "gmp", "version": "6.3.0",
        "url": "https://ftp.gnu.org/gnu/gmp/gmp-6.3.0.tar.xz",
        "sha256": "",
        "build": "autotools", "keg_only": False,
        "desc": "GNU Multiple Precision — 大整数有理数运算库",
        "homepage": "https://gmplib.org/",
        "deps": [],
    },
    {
        "name": "mpfr", "version": "4.2.2",
        "url": "https://ftp.gnu.org/gnu/mpfr/mpfr-4.2.2.tar.xz",
        "sha256": "",
        "build": "autotools", "keg_only": False,
        "desc": "GNU MPFR — 多精度浮点数运算",
        "homepage": "https://www.mpfr.org/",
        "deps": ["gmp"],
    },
    {
        "name": "mpc", "version": "1.3.1",
        "url": "https://ftp.gnu.org/gnu/mpc/mpc-1.3.1.tar.gz",
        "sha256": "",
        "build": "autotools", "keg_only": False,
        "desc": "GNU MPC — 多精度复数运算",
        "homepage": "https://www.multiprecision.org/mpc/",
        "deps": ["gmp", "mpfr"],
    },
    {
        "name": "isl", "version": "0.27",
        "url": "https://libisl.sourceforge.io/isl-0.27.tar.xz",
        "sha256": "",
        "build": "autotools", "keg_only": True,
        "desc": "Integer Set Library — 多面体依赖分析（GCC Graphite）",
        "homepage": "https://libisl.sourceforge.io/",
        "deps": ["gmp"],
    },

    # --- glib / qemu 依赖 ---
    {
        "name": "libffi", "version": "3.4.7",
        "url": "https://github.com/libffi/libffi/releases/download/v3.4.7/libffi-3.4.7.tar.gz",
        "sha256": "",
        "build": "autotools", "keg_only": False,
        "desc": "libffi — Foreign Function Interface 调用库",
        "homepage": "https://github.com/libffi/libffi",
        "deps": [],
    },
    {
        "name": "gettext", "version": "0.23",
        "url": "https://ftp.gnu.org/gnu/gettext/gettext-0.23.tar.xz",
        "sha256": "",
        "build": "autotools", "keg_only": True,
        "desc": "GNU gettext — i18n/l10n 工具链",
        "homepage": "https://www.gnu.org/software/gettext/",
        "deps": [],
    },
    {
        "name": "pcre2", "version": "10.45",
        "url": "https://github.com/PCRE2Project/pcre2/releases/download/pcre2-10.45/pcre2-10.45.tar.gz",
        "sha256": "",
        "build": "cmake", "keg_only": False,
        "desc": "PCRE2 — 兼容 Perl 的正则表达式库",
        "homepage": "https://pcre2project.github.io/pcre2/",
        "deps": [],
    },
    {
        "name": "pixman", "version": "0.43.2",
        "url": "https://www.cairographics.org/releases/pixman-0.43.2.tar.gz",
        "sha256": "",
        "build": "autotools", "keg_only": False,
        "desc": "pixman — 像素操作基础库（Cairo/SDL/QEMU 依赖）",
        "homepage": "https://pixman.org/",
        "deps": [],
    },
    {
        "name": "slirp", "version": "4.8.0",
        "url": "https://gitlab.freedesktop.org/slirp/slirp/-/archive/v4.8.0/slirp-v4.8.0.tar.gz",
        "sha256": "",
        "build": "cmake", "keg_only": False,
        "desc": "slirp — 用户态 TCP/IP 网络栈（QEMU 网络后端）",
        "homepage": "https://slirp.rs/",
        "deps": [],
    },
]


def generate_toml(d: dict) -> str:
    lines = []
    lines.append(f"[package]")
    lines.append(f'name = "{d["name"]}"')
    lines.append(f'version = "{d["version"]}"')
    lines.append(f'description = "{d["desc"]}"')
    lines.append(f'homepage = "{d["homepage"]}"')
    if d["deps"]:
        deps = ", ".join(f'"{x}"' for x in d["deps"])
        lines.append(f"dependencies = [{deps}]")
    lines.append("")
    lines.append("[source]")
    lines.append(f'url = "{d["url"]}"')
    lines.append(f'sha256 = "{d["sha256"]}"')
    lines.append("")
    lines.append("[build]")
    lines.append(f'system = "{d["build"]}"')
    lines.append("")
    lines.append("[install]")
    lines.append(f'keg_only = {"true" if d["keg_only"] else "false"}')
    lines.append("")
    return "\n".join(lines)


def main():
    out_dir = Path("Formula")
    out_dir.mkdir(parents=True, exist_ok=True)
    cache = Path("/tmp/fb-dep-cache")
    cache.mkdir(exist_ok=True)

    for d in DEPS:
        name = d["name"]
        print(f"=== {name} {d['version']} ===", flush=True)
        out = out_dir / f"{name}.toml"

        # 若已存在且 sha256 非空，跳过
        if out.exists():
            text = out.read_text()
            if 'sha256 = ""' not in text:
                print(f"  ✅ 已存在且 sha256 非空，跳过")
                continue

        # 下载 + 算 sha256
        fname = d["url"].rsplit("/", 1)[-1]
        fpath = cache / fname
        try:
            if not fpath.exists() or fpath.stat().st_size == 0:
                print(f"  下载中...", end=" ", flush=True)
                d["sha256"] = download(d["url"], fpath)
                print(f"OK ({fpath.stat().st_size // 1024} KB)")
            else:
                d["sha256"] = sha256_file(fpath)
                print(f"  复用缓存，sha256={d['sha256'][:12]}...")
        except Exception as e:
            print(f"  ❌ 下载失败: {e}")
            d["sha256"] = ""  # 允许空 sha256

        toml = generate_toml(d)
        out.write_text(toml, encoding="utf-8")
        print(f"  ✅ 写入 {out}")


if __name__ == "__main__":
    main()
