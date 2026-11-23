#!/usr/bin/env python3
"""Local stand-in for GitHub Releases, for testing installs and self-updates.

Serves the releases published by ``publish.py`` (``releases/<tag>/``) with
the same URL shapes and JSON fields Forge and the install docs use:

  GET /repos/<owner>/<repo>/releases/latest          latest release (JSON)
  GET /repos/<owner>/<repo>/releases                 all releases, newest first
  GET /repos/<owner>/<repo>/releases/tags/<tag>      one release
  GET /repos/<owner>/<repo>/releases/assets/<id>     asset bytes (API style)
  GET /<owner>/<repo>/releases/download/<tag>/<name> asset bytes
  GET /<owner>/<repo>/releases/latest/download/<name>  302 to the latest tag

Like a private GitHub repository it understands a personal access token
(``Authorization: token …`` or ``Bearer …``), but any value is accepted. With
``--require-token`` requests without one get 404, as GitHub answers for a
private repository. Forge's updater sends no token, so leave that off when
testing self-update.

Files are read from disk on every request: publishing a release needs no
restart. Standard library only.

    py scripts/setup_testing/feed_server.py                 # 10.50.0.1:8000
    py scripts/setup_testing/feed_server.py --bind 0.0.0.0 --port 8000
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import sys
import zlib
from datetime import datetime, timezone
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import unquote, urlsplit

HERE = Path(__file__).resolve().parent
NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._+~-]{0,127}$")


def asset_id(tag: str, name: str) -> int:
    return zlib.crc32(f"{tag}/{name}".encode()) & 0x7FFFFFFF


class Feed:
    def __init__(self, root: Path, owner: str, repo: str) -> None:
        self.root, self.owner, self.repo = root, owner, repo

    def release_dirs(self) -> list[Path]:
        """Published releases (a directory with release.json), newest first."""
        if not self.root.is_dir():
            return []
        found = []
        for d in self.root.iterdir():
            meta = d / "release.json"
            if d.is_dir() and not d.name.startswith(".") and NAME.match(d.name) and meta.is_file():
                try:
                    published = json.loads(meta.read_text(encoding="utf-8")).get("published_at", "")
                except (OSError, ValueError):
                    continue
                found.append((published, d))
        return [d for _, d in sorted(found, key=lambda x: x[0], reverse=True)]

    def release(self, d: Path, base: str) -> dict:
        meta = json.loads((d / "release.json").read_text(encoding="utf-8"))
        tag = d.name
        api = f"{base}/repos/{self.owner}/{self.repo}"
        assets = []
        for f in sorted(d.iterdir()):
            if f.is_file() and f.name != "release.json" and NAME.match(f.name):
                aid = asset_id(tag, f.name)
                st = f.stat()
                assets.append(
                    {
                        "id": aid,
                        "name": f.name,
                        "label": "",
                        "content_type": "application/octet-stream",
                        "state": "uploaded",
                        "size": st.st_size,
                        "created_at": meta.get("published_at"),
                        "updated_at": meta.get("published_at"),
                        "url": f"{api}/releases/assets/{aid}",
                        "browser_download_url": f"{base}/{self.owner}/{self.repo}/releases/download/{tag}/{f.name}",
                    }
                )
        return {
            "id": asset_id(tag, ""),
            "tag_name": tag,
            "name": meta.get("name") or tag,
            "body": meta.get("body", ""),
            "draft": False,
            "prerelease": False,
            "created_at": meta.get("published_at"),
            "published_at": meta.get("published_at"),
            "html_url": f"{base}/{self.owner}/{self.repo}/releases/tag/{tag}",
            "url": f"{api}/releases/{asset_id(tag, '')}",
            "assets_url": f"{api}/releases/{asset_id(tag, '')}/assets",
            "assets": assets,
        }

    def find_asset(self, aid: int) -> Path | None:
        for d in self.release_dirs():
            for f in d.iterdir():
                if f.is_file() and f.name != "release.json" and asset_id(d.name, f.name) == aid:
                    return f
        return None


class Handler(BaseHTTPRequestHandler):
    server_version = "forge-local-feed/1"
    feed: Feed
    require_token: bool

    # Paths only: tokens and query strings never reach the log.
    def log_message(self, fmt: str, *args) -> None:
        sys.stderr.write(f"{datetime.now():%H:%M:%S} {self.client_address[0]} {fmt % args}\n")

    def log_request(self, code="-", size="-") -> None:
        auth = "token" if self.headers.get("Authorization") else "anon"
        path = urlsplit(self.path).path
        self.log_message('"%s %s" %s %s', self.command, path, str(code), auth)

    def base(self) -> str:
        host = self.headers.get("Host") or f"{self.server.server_address[0]}:{self.server.server_address[1]}"
        return f"http://{host}"

    def send_json(self, status: int, doc) -> None:
        body = json.dumps(doc, indent=2).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def not_found(self) -> None:
        self.send_json(HTTPStatus.NOT_FOUND, {"message": "Not Found", "status": "404"})

    def send_file(self, f: Path) -> None:
        self.send_response(HTTPStatus.OK)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(f.stat().st_size))
        self.send_header("Content-Disposition", f'attachment; filename="{f.name}"')
        self.end_headers()
        if self.command != "HEAD":
            with f.open("rb") as src:
                shutil.copyfileobj(src, self.wfile, 1024 * 1024)

    def authorized(self) -> bool:
        auth = self.headers.get("Authorization", "")
        has = bool(re.match(r"^(token|bearer)\s+\S+", auth, re.IGNORECASE))
        return has or not self.require_token

    def do_HEAD(self) -> None:
        self.do_GET()

    def do_GET(self) -> None:
        if not self.authorized():
            return self.not_found()
        parts = [unquote(p) for p in urlsplit(self.path).path.split("/") if p]
        feed = self.feed
        if any(p in (".", "..") or "/" in p or "\\" in p for p in parts):
            return self.not_found()
        try:
            # /repos/<owner>/<repo>/releases[...]
            if parts[:4] == ["repos", feed.owner, feed.repo, "releases"]:
                rest = parts[4:]
                dirs = feed.release_dirs()
                if rest == []:
                    return self.send_json(200, [feed.release(d, self.base()) for d in dirs])
                if rest == ["latest"]:
                    return self.send_json(200, feed.release(dirs[0], self.base())) if dirs else self.not_found()
                if len(rest) == 2 and rest[0] == "tags" and NAME.match(rest[1]):
                    d = feed.root / rest[1]
                    return self.send_json(200, feed.release(d, self.base())) if d in dirs else self.not_found()
                if len(rest) == 2 and rest[0] == "assets" and rest[1].isdigit():
                    f = feed.find_asset(int(rest[1]))
                    return self.send_file(f) if f else self.not_found()
                return self.not_found()
            # /<owner>/<repo>/releases/latest/download/<name>
            if parts[:3] == [feed.owner, feed.repo, "releases"] and parts[3:5] == ["latest", "download"]:
                dirs = feed.release_dirs()
                if len(parts) != 6 or not dirs or not NAME.match(parts[5]):
                    return self.not_found()
                self.send_response(HTTPStatus.FOUND)
                self.send_header("Location", f"/{feed.owner}/{feed.repo}/releases/download/{dirs[0].name}/{parts[5]}")
                self.send_header("Content-Length", "0")
                self.end_headers()
                return None
            # /<owner>/<repo>/releases/download/<tag>/<name>
            if parts[:4] == [feed.owner, feed.repo, "releases", "download"] and len(parts) == 6:
                tag, name = parts[4], parts[5]
                if not (NAME.match(tag) and NAME.match(name)) or name == "release.json":
                    return self.not_found()
                d = feed.root / tag
                f = d / name
                if d in feed.release_dirs() and f.is_file():
                    return self.send_file(f)
                return self.not_found()
            if parts == []:
                dirs = feed.release_dirs()
                return self.send_json(
                    200,
                    {
                        "feed_url": f"{self.base()}/repos/{feed.owner}/{feed.repo}/releases/latest",
                        "latest": dirs[0].name if dirs else None,
                        "releases": [d.name for d in dirs],
                    },
                )
            return self.not_found()
        except (BrokenPipeError, ConnectionResetError):
            return None


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--bind", default="10.50.0.1", help="address to listen on (default: the ForgeLab host IP)")
    ap.add_argument("--port", type=int, default=8000)
    ap.add_argument("--owner", default="orbyntech")
    ap.add_argument("--repo", default="forge")
    ap.add_argument("--root", type=Path, default=HERE / "releases", help="published releases directory")
    ap.add_argument("--require-token", action="store_true", help="404 without an Authorization header, like a private repo")
    args = ap.parse_args()

    Handler.feed = Feed(args.root.resolve(), args.owner, args.repo)
    Handler.require_token = args.require_token
    try:
        srv = ThreadingHTTPServer((args.bind, args.port), Handler)
    except OSError as e:
        print(f"cannot listen on {args.bind}:{args.port}: {e}", file=sys.stderr)
        if args.bind == "10.50.0.1":
            print("Is the ForgeLab switch up (vEthernet (ForgeLab) = 10.50.0.1)?", file=sys.stderr)
        return 1
    base = f"http://{args.bind}:{args.port}"
    print(f"Local release feed on {base}  (releases: {Handler.feed.root})")
    print(f"  feed_url = \"{base}/repos/{args.owner}/{args.repo}/releases/latest\"")
    print(f"  started {datetime.now(timezone.utc):%Y-%m-%d %H:%M:%SZ}; Ctrl+C to stop")
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
