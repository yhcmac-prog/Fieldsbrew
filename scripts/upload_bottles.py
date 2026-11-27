#!/usr/bin/env python3
"""
批量上传 bottle 到 GitHub Release + 更新 fields-core formula。

用法：
  python3 scripts/upload_bottles.py --bottles /tmp/fb-bottles \
                                    --repo yhcmac-prog/fields-core \
                                    --tag bottles-2026-11-22 \
                                    --formulas /tmp/fc-formulas

需要：GITHUB_TOKEN 环境变量（或 prompt 输入），用于 GitHub API 鉴权。

GitHub API 端点：
  POST   /repos/{owner}/{repo}/releases
  POST   /repos/{owner}/{repo}/releases/{id}/assets?name=<filename>
"""
import argparse, base64, hashlib, json, os, re, sys, time, urllib.request, urllib.error

GH_API = "https://api.github.com"

def gh_request(method, path, token, body=None, accept="application/vnd.github+json", raw=False):
    url = f"{GH_API}{path}"
    data = None
    headers = {
        "Accept": accept,
        "User-Agent": "fieldsbrew-bottles/1.0",
        "X-GitHub-Api-Version": "2022-11-28",
    }
    if token:
        headers["Authorization"] = f"Bearer {token}"
    if body is not None:
        data = json.dumps(body).encode() if isinstance(body, (dict, list)) else body
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        resp = urllib.request.urlopen(req, timeout=120)
        ct = resp.headers.get("Content-Type", "")
        content = resp.read()
        if raw:
            return resp, content
        if "application/json" in ct or content.startswith(b"{"):
            return resp, json.loads(content)
        return resp, content
    except urllib.error.HTTPError as e:
        ct = e.headers.get("Content-Type", "") if hasattr(e, "headers") else ""
        body_text = e.read().decode(errors="replace") if hasattr(e, "read") else str(e)
        print(f"  HTTP {e.code} {method} {path}: {body_text[:200]}", file=sys.stderr)
        raise

def get_or_create_release(owner, repo, tag, desc, token):
    """Get existing release by tag, or create new one."""
    # Try get by tag
    try:
        _, r = gh_request("GET", f"/repos/{owner}/{repo}/releases/tags/{tag}", token)
        print(f"  已存在 release: {r['html_url']}")
        return r
    except urllib.error.HTTPError as e:
        if e.code != 404:
            raise

    # Create
    print(f"  创建 release: {tag}")
    _, r = gh_request("POST", f"/repos/{owner}/{repo}/releases", token, body={
        "tag_name": tag,
        "target_commitish": "main",
        "name": f"Bottles {tag}",
        "body": desc,
        "draft": False,
        "prerelease": False,
        "generate_release_notes": False,
    })
    print(f"  新 release: {r['html_url']}")
    return r

def upload_asset(release, filepath, token):
    """Upload a file as release asset (raw binary PUT)."""
    name = os.path.basename(filepath)
    upload_url = release["upload_url"].split("{")[0]  # strip {?name,label}
    url = f"{upload_url}?name={urllib.parse.quote(name)}"
    with open(filepath, "rb") as f:
        data = f.read()
    headers = {
        "Content-Type": "application/gzip",
        "User-Agent": "fieldsbrew-bottles/1.0",
        "X-GitHub-Api-Version": "2022-11-28",
    }
    if token:
        headers["Authorization"] = f"Bearer {token}"
    req = urllib.request.Request(url, data=data, headers=headers, method="POST")
    resp = urllib.request.urlopen(req, timeout=180)
    asset = json.loads(resp.read())
    print(f"    ✓ {name} ({len(data)//1024} KB) -> {asset['browser_download_url']}")
    return asset

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bottles", required=True, help="bottle 目录（含 summary.json 和 *.tar.gz）")
    ap.add_argument("--repo", required=True, help="owner/repo，如 yhcmac-prog/fields-core")
    ap.add_argument("--tag", default=f"bottles-{time.strftime('%Y-%m-%d')}")
    ap.add_argument("--token", default=None, help="GitHub token，省略时读 $GITHUB_TOKEN")
    args = ap.parse_args()

    token = args.token or os.environ.get("GITHUB_TOKEN", "").strip()
    if not token:
        print("错误: 需要 GitHub token（--token 或 $GITHUB_TOKEN）", file=sys.stderr)
        sys.exit(1)

    owner, repo = args.repo.split("/", 1)
    summary_path = os.path.join(args.bottles, "summary.json")
    if not os.path.exists(summary_path):
        print(f"错误: 找不到 {summary_path}", file=sys.stderr)
        sys.exit(1)
    summary = json.loads(open(summary_path).read())

    tag = args.tag
    desc_lines = [f"自动生成的 x86_64 bottle（共 {len(summary)} 个包）", ""]
    for b in summary:
        desc_lines.append(f"- **{b['name']}@{b['version']}** ({b['size_mb']:.1f} MB) — `{b['sha256'][:16]}...`")
    desc = "\n".join(desc_lines)

    print(f"=== GitHub Release: {owner}/{repo} tag={tag} ===")
    release = get_or_create_release(owner, repo, tag, desc, token)
    print(f"\n=== 上传 {len(summary)} 个 bottle 资产 ===")

    updated_summary = []
    for b in summary:
        fp = b["path"]
        if not os.path.exists(fp):
            print(f"  ✗ 跳过（文件不存在）: {fp}", file=sys.stderr)
            continue
        try:
            asset = upload_asset(release, fp, token)
            b["browser_download_url"] = asset["browser_download_url"]
            b["asset_id"] = asset["id"]
            updated_summary.append(b)
        except Exception as e:
            print(f"  ✗ 上传失败 {os.path.basename(fp)}: {e}", file=sys.stderr)

    # 更新 summary.json（加上 asset URL）
    with open(summary_path, "w") as f:
        json.dump(updated_summary, f, indent=2)
    print(f"\n==> 完成 {len(updated_summary)}/{len(summary)} 个 bottle")
    print(f"==> Release URL: {release['html_url']}")
    print(f"==> 更新后的 summary.json: {summary_path}")

if __name__ == "__main__":
    import urllib.parse
    main()
