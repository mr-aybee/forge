"""A stand-in for GitHub on the lab: release assets and a git repository over HTTPS.

    python mock_github.py --bind 10.50.0.1 --port 8443

Serves, from `--root` (default `.lab/mock-github`):

* `GET /<owner>/<repo>/releases/download/<tag>/<file>`: a release asset, the
  path GitHub uses (`https://github.com/<owner>/<repo>/releases/download/…`);
* `/<owner>/<repo>.git/…`: a git repository over smart HTTP (`git
  http-backend`), so a node can `git fetch --depth 1 <repo> <commit>`;
* `GET /healthz`.

TLS uses a certificate signed by the lab CA (`.lab/ca/`), made on first
start with `openssl` (on Windows the one that ships with Git for Windows
works). Nodes trust the CA once (`10-deploy-setup.py` installs it), the way a
company trusts its private artifact host.

Failure knobs for the scenarios: `?delay=<seconds>` slows an answer, and any
file under `broken/` is served with its last byte changed (a checksum
mismatch).
"""

from __future__ import annotations

import argparse
import http.server
import os
import shutil
import ssl
import subprocess
import sys
import time
import urllib.parse
from pathlib import Path

HERE = Path(__file__).resolve().parent
LAB = HERE.parent.parent / ".lab"
DEFAULT_ROOT = LAB / "mock-github"
CA_DIR = LAB / "ca"


def openssl() -> str:
    found = shutil.which("openssl")
    if found:
        return found
    for p in (r"C:\Program Files\Git\usr\bin\openssl.exe", r"C:\Program Files\Git\mingw64\bin\openssl.exe"):
        if Path(p).exists():
            return p
    sys.exit("openssl not found: install it (Linux) or Git for Windows (Windows)")


def ensure_certs(host: str) -> tuple[Path, Path, Path]:
    """The lab CA and a server certificate for `host` (IP or name). Returns (ca, cert, key)."""
    CA_DIR.mkdir(parents=True, exist_ok=True)
    ca_key, ca_crt = CA_DIR / "lab-ca.key", CA_DIR / "lab-ca.crt"
    key, crt = CA_DIR / f"mock-{host}.key", CA_DIR / f"mock-{host}.crt"
    o = openssl()
    if not ca_crt.exists():
        subprocess.run([o, "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", str(ca_key), "-out", str(ca_crt),
                        "-days", "3650", "-subj", "/CN=Forge lab CA",
                        # Strict verifiers (OpenSSL 3.4+, Python 3.13+) need a proper CA profile.
                        "-addext", "basicConstraints=critical,CA:TRUE",
                        "-addext", "keyUsage=critical,keyCertSign,cRLSign",
                        "-addext", "subjectKeyIdentifier=hash"], check=True, capture_output=True)
    if not crt.exists():
        csr = CA_DIR / f"mock-{host}.csr"
        ext = CA_DIR / f"mock-{host}.ext"
        san = f"IP:{host}" if host.replace(".", "").isdigit() or ":" in host else f"DNS:{host}"
        ext.write_text(
            f"subjectAltName={san},DNS:github.lab\nbasicConstraints=CA:FALSE\n"
            "keyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n"
            "authorityKeyIdentifier=keyid\nsubjectKeyIdentifier=hash\n"
        )
        subprocess.run([o, "req", "-newkey", "rsa:2048", "-nodes", "-keyout", str(key), "-out", str(csr),
                        "-subj", f"/CN={host}"], check=True, capture_output=True)
        subprocess.run([o, "x509", "-req", "-in", str(csr), "-CA", str(ca_crt), "-CAkey", str(ca_key), "-CAcreateserial",
                        "-out", str(crt), "-days", "825", "-extfile", str(ext)], check=True, capture_output=True)
    return ca_crt, crt, key


def git_http_backend() -> list[str]:
    """How to run `git http-backend` on this machine."""
    exec_path = subprocess.run(["git", "--exec-path"], capture_output=True, text=True, check=True).stdout.strip()
    for name in ("git-http-backend", "git-http-backend.exe"):
        p = Path(exec_path) / name
        if p.exists():
            return [str(p)]
    return ["git", "http-backend"]


class Handler(http.server.BaseHTTPRequestHandler):
    root: Path = DEFAULT_ROOT
    backend: list[str] = []

    def log_message(self, fmt: str, *args) -> None:  # noqa: A003 - stdlib name
        sys.stderr.write(f"{time.strftime('%H:%M:%S')} {self.address_string()} {fmt % args}\n")

    def _delay(self, query: dict) -> None:
        if "delay" in query:
            time.sleep(min(float(query["delay"][0]), 60))

    def do_GET(self) -> None:  # noqa: N802 - stdlib name
        self._route()

    def do_POST(self) -> None:  # noqa: N802 - stdlib name
        self._route()

    def _route(self) -> None:
        url = urllib.parse.urlsplit(self.path)
        query = urllib.parse.parse_qs(url.query)
        self._delay(query)
        path = urllib.parse.unquote(url.path)
        if path == "/healthz":
            return self._send(200, b"ok\n", "text/plain")
        if ".git/" in path or path.endswith(".git"):
            return self._git(url)
        parts = [p for p in path.split("/") if p]
        # /<owner>/<repo>/releases/download/<tag>/<file>
        if len(parts) == 6 and parts[2:4] == ["releases", "download"] and not any(p in ("..", ".") for p in parts):
            f = self.root / "releases" / parts[0] / parts[1] / parts[4] / parts[5]
            if f.is_file():
                data = f.read_bytes()
                if parts[4] == "broken" or "broken" in parts[5]:
                    data = data[:-1] + bytes([data[-1] ^ 0xFF]) if data else data
                return self._send(200, data, "application/octet-stream")
        return self._send(404, b"Not Found\n", "text/plain")

    def _send(self, code: int, body: bytes, ctype: str) -> None:
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def _git(self, url: urllib.parse.SplitResult) -> None:
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length else b""
        env = dict(os.environ)
        env.update({
            "GIT_PROJECT_ROOT": str(self.root / "git"),
            "GIT_HTTP_EXPORT_ALL": "1",
            "PATH_INFO": urllib.parse.unquote(url.path),
            "QUERY_STRING": url.query,
            "REQUEST_METHOD": self.command,
            "CONTENT_TYPE": self.headers.get("Content-Type", ""),
            "CONTENT_LENGTH": str(len(body)),
            "REMOTE_ADDR": self.client_address[0],
            "HTTP_GIT_PROTOCOL": self.headers.get("Git-Protocol", ""),
        })
        p = subprocess.run(self.backend, input=body, capture_output=True, env=env)
        head, _, payload = p.stdout.partition(b"\r\n\r\n")
        if not _:
            head, _, payload = p.stdout.partition(b"\n\n")
        status = 200
        headers = []
        for line in head.decode(errors="replace").splitlines():
            k, _, v = line.partition(":")
            if k.lower() == "status":
                status = int(v.strip().split()[0])
            elif k:
                headers.append((k.strip(), v.strip()))
        self.send_response(status)
        for k, v in headers:
            self.send_header(k, v)
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--bind", default="10.50.0.1")
    p.add_argument("--port", type=int, default=8443)
    p.add_argument("--root", type=Path, default=DEFAULT_ROOT)
    p.add_argument("--host", default=None, help="name or IP in the certificate (default: --bind)")
    a = p.parse_args()
    _, crt, key = ensure_certs(a.host or a.bind)
    Handler.root = a.root
    Handler.backend = git_http_backend()
    srv = http.server.ThreadingHTTPServer((a.bind, a.port), Handler)
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain(str(crt), str(key))
    srv.socket = ctx.wrap_socket(srv.socket, server_side=True)
    print(f"mock GitHub on https://{a.bind}:{a.port} (root {a.root})", flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()
