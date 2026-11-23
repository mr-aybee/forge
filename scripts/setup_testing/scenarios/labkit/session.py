"""Signs in to Forge with a username and password (for test users), stdlib only.

The scripts themselves use an API key; this is for checking what *another*
user may do (scenario 06). It solves the login proof-of-work challenge
(D-064) as the console does.
"""

from __future__ import annotations

import hashlib
import http.cookiejar
import json
import urllib.error
import urllib.parse
import urllib.request


class Session:
    def __init__(self, base: str, login: str, password: str):
        self.base = base.rstrip("/")
        self.opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
        self.csrf: str | None = None
        body = {"login": login, "password": password, **self._challenge(login)}
        status, doc = self.call("POST", "/api/v1/auth/login", body)
        if status != 200:
            raise RuntimeError(f"sign-in as {login} failed: {status} {doc}")
        self.csrf = doc.get("csrf_token")

    def _challenge(self, login: str) -> dict:
        url = f"{self.base}/api/v1/auth/challenge?login={urllib.parse.quote(login)}"
        try:
            with urllib.request.urlopen(url, timeout=30) as r:
                c = json.loads(r.read())
        except (urllib.error.URLError, ValueError):
            return {}
        if not c.get("required"):
            return {}
        salt, bits, n = bytes.fromhex(c["salt"]), int(c["difficulty"]), 0
        while int.from_bytes(hashlib.sha256(salt + str(n).encode()).digest(), "big") >> (256 - bits):
            n += 1
        return {"challenge_id": c["id"], "nonce": str(n)}

    def call(self, method: str, path: str, body: object | None = None) -> tuple[int, object]:
        """(status, JSON body or text)."""
        req = urllib.request.Request(self.base + path, method=method)
        req.add_header("Content-Type", "application/json")
        if self.csrf:
            req.add_header("X-CSRF-Token", self.csrf)
        data = json.dumps(body).encode() if body is not None else None
        try:
            with self.opener.open(req, data=data, timeout=60) as r:
                raw, status = r.read(), r.status
        except urllib.error.HTTPError as e:
            raw, status = e.read(), e.code
        try:
            return status, json.loads(raw) if raw else None
        except ValueError:
            return status, raw.decode(errors="replace")
