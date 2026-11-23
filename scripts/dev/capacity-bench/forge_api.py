"""Minimal Forge API client for the capacity benchmark (standard library only)."""
import hashlib, http.cookiejar, json, urllib.parse, urllib.request


def _solve(salt_hex, bits):
    salt, n = bytes.fromhex(salt_hex), 0
    while True:
        d = hashlib.sha256(salt + str(n).encode()).digest()
        v = int.from_bytes(d, "big")
        if v >> (256 - bits) == 0 if bits else True:
            return str(n)
        n += 1


class Api:
    """A signed-in session. `get` returns (status, seconds, parsed JSON or None)."""

    def __init__(self, base, login, password, timeout=30):
        self.base, self.timeout = base.rstrip("/"), timeout
        self.jar = http.cookiejar.CookieJar()
        self.op = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(self.jar))
        body = {"login": login, "password": password}
        _, _, c = self.get("/api/v1/auth/challenge?" + urllib.parse.urlencode({"login": login}))
        if c and c.get("required"):
            body["challenge_id"] = c["id"]
            body["nonce"] = _solve(c["salt"], int(c["difficulty"]))
        req = urllib.request.Request(self.base + "/api/v1/auth/login", json.dumps(body).encode(),
                                     {"Content-Type": "application/json"})
        r = json.loads(self.op.open(req, timeout=timeout).read())
        if r.get("mfa_required"):
            raise SystemExit("the benchmark user has MFA on; turn it off for this user")

    def get(self, path):
        import time
        t = time.monotonic()
        try:
            with self.op.open(self.base + path, timeout=self.timeout) as r:
                data = r.read()
                status = r.status
        except urllib.error.HTTPError as e:
            data, status = e.read(), e.code
        except Exception:  # timeout, reset: counted as status 0
            return 0, time.monotonic() - t, None
        dt = time.monotonic() - t
        try:
            return status, dt, json.loads(data) if data else None
        except ValueError:
            return status, dt, None
