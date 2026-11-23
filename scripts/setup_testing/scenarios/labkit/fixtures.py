"""Builds the artifacts the deployment scenarios publish on the mock GitHub.

    python fixtures.py            # writes .lab/mock-github and prints a summary

* `acme/hello-py` releases `v1`, `v2`, `v3`: a small HTTP service (Python
  stdlib) listening on `$PORT` (default 8090). `GET /` answers
  `hello-py <version>`; `GET /ready` answers 200, except in `v3`, which is
  never ready (503). Each release has `hello-py-<v>.tar.gz` (files at the
  top) and `hello-py-<v>.zip` (inside a `hello-py-<v>/` folder, as GitHub
  source archives are: unpack with `stripComponents: 1`).
* `acme/hello-sh` release `v1`: a job, `run.sh`, that prints a line and exits.
* `acme/hello-py.git`: the same code as a git repository, one commit per
  version (tags `v1`, `v2`, `v3`), fetchable by commit over smart HTTP.
* `broken` copies of v2 under the tag `broken`: the mock serves them with one
  byte changed, so their SHA-256 never matches.

Returns (and stores in `.lab/deploy-lab.json` under `fixtures`) every URL
path, SHA-256 and commit the scenarios need.
"""

from __future__ import annotations

import hashlib
import io
import json
import os
import shutil
import stat
import subprocess
import tarfile
import tempfile
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent / ".lab" / "mock-github"

APP = '''import http.server, os
VERSION = "{version}"
READY = {ready}

class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/ready":
            code, body = (200, "ready\\n") if READY else (503, "not ready\\n")
        else:
            code, body = 200, "hello-py " + VERSION + "\\n"
        data = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *a):
        pass

port = int(os.environ.get("PORT", "8090"))
http.server.ThreadingHTTPServer(("0.0.0.0", port), H).serve_forever()
'''

JOB = '#!/bin/sh\necho "hello-sh {version} ran at $(date -u +%FT%TZ)"\n'


def files_for(version: str) -> dict[str, str]:
    return {
        "app.py": APP.format(version=version, ready="False" if version == "v3" else "True"),
        "VERSION": version + "\n",
    }


def _tar(files: dict[str, str], prefix: str = "") -> bytes:
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:gz") as t:
        for name, text in files.items():
            data = text.encode()
            info = tarfile.TarInfo(prefix + name)
            info.size, info.mode, info.mtime = len(data), 0o644, 1_700_000_000
            t.addfile(info, io.BytesIO(data))
    return buf.getvalue()


def _zip(files: dict[str, str], prefix: str) -> bytes:
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
        for name, text in files.items():
            z.writestr(prefix + name, text)
    return buf.getvalue()


def _put(owner: str, repo: str, tag: str, name: str, data: bytes) -> dict:
    d = ROOT / "releases" / owner / repo / tag
    d.mkdir(parents=True, exist_ok=True)
    (d / name).write_bytes(data)
    return {"path": f"/{owner}/{repo}/releases/download/{tag}/{name}", "sha256": hashlib.sha256(data).hexdigest()}


def _git(*args: str, cwd: Path) -> str:
    env_args = ["-c", "user.name=Forge lab", "-c", "user.email=lab@example.invalid", "-c", "commit.gpgsign=false"]
    return subprocess.run(["git", *env_args, *args], cwd=cwd, check=True, capture_output=True, text=True).stdout.strip()


def build() -> dict:
    if ROOT.exists():
        # Git marks its objects read-only (Windows refuses to delete them then).
        def writable(func, path, _exc):
            os.chmod(path, stat.S_IWRITE)
            func(path)

        shutil.rmtree(ROOT, onexc=writable)
    out: dict = {"hello-py": {}, "hello-sh": {}, "git": {}}
    for v in ("v1", "v2", "v3"):
        f = files_for(v)
        out["hello-py"][v] = {
            "tar": _put("acme", "hello-py", v, f"hello-py-{v}.tar.gz", _tar(f)),
            "zip": _put("acme", "hello-py", v, f"hello-py-{v}.zip", _zip(f, f"hello-py-{v}/")),
        }
    # A checksum that never matches: the mock flips a byte of anything under `broken`.
    good = _tar(files_for("v2"))
    out["hello-py"]["broken"] = {"tar": _put("acme", "hello-py", "broken", "hello-py-v2.tar.gz", good)}
    out["hello-py"]["broken"]["tar"]["sha256"] = hashlib.sha256(good).hexdigest()
    out["hello-sh"]["v1"] = {"tar": _put("acme", "hello-sh", "v1", "hello-sh-v1.tar.gz", _tar({"run.sh": JOB.format(version="v1")}))}

    # The git repository: one commit per version.
    bare = ROOT / "git" / "acme" / "hello-py.git"
    bare.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(["git", "init", "-q", "--bare", str(bare)], check=True)
    _git("config", "http.receivepack", "false", cwd=bare)
    _git("config", "uploadpack.allowAnySHA1InWant", "true", cwd=bare)
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        _git("init", "-q", "-b", "main", cwd=work)
        for v in ("v1", "v2", "v3"):
            for name, text in files_for(v).items():
                (work / name).write_text(text, newline="\n")
            _git("add", "-A", cwd=work)
            _git("commit", "-q", "-m", f"hello-py {v}", cwd=work)
            _git("tag", v, cwd=work)
            out["git"][v] = _git("rev-parse", "HEAD", cwd=work)
        _git("push", "-q", "--tags", str(bare), "main", cwd=work)
    out["git"]["repo_path"] = "/acme/hello-py.git"
    return out


if __name__ == "__main__":
    print(json.dumps(build(), indent=2))
