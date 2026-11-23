#!/usr/bin/env python3
"""Mock portal (Python): a small multi-page app that says which node served it.

Run by Forge as a service. Port from $PORT (default 8101). Pages:

    /         home; `/?format=json` answers JSON for test scripts
    /notes    notes stored in PostgreSQL (GET lists, POST adds)
    /health   JSON health: process and database
    /about    node, addresses and request headers

Every page has a footer with the node name and sends `X-Served-By`, so load
balancing is visible in the browser and with curl.

Optional, by environment variable:

    DATABASE_URL   postgres://user:password@host:5432/db  (needs psycopg; the
                   standard library alone runs without it)
    FORGE_URL      Forge server base URL, for example http://172.17.0.1:55130
    FORGE_LOG_KEY  log ingest key (flk_...): sends structured logs to Forge's
                   Seq-compatible endpoint
"""
import datetime
import html
import http.server
import json
import os
import platform
import queue
import socket
import subprocess
import threading
import time
import urllib.parse
import urllib.request

PORT = int(os.environ.get("PORT", "8101"))
NODE = socket.gethostname()
RUNTIME = f"Python {platform.python_version()}"
DATABASE_URL = os.environ.get("DATABASE_URL", "")
FORGE_URL = os.environ.get("FORGE_URL", "").rstrip("/")
FORGE_LOG_KEY = os.environ.get("FORGE_LOG_KEY", "")
HITS = 0
HITS_LOCK = threading.Lock()

try:
    import psycopg  # type: ignore
except ImportError:  # the lab runs the portal with the standard library only
    psycopg = None


# ------------------------------------------------------------------ logging
class SeqLog:
    """Batches CLEF events and posts them to Forge's /api/events/raw."""

    def __init__(self, base, key):
        self.url = f"{base}/api/events/raw?clef" if base and key else ""
        self.key = key
        self.q = queue.Queue(maxsize=10000)
        if self.url:
            threading.Thread(target=self._pump, daemon=True).start()

    def event(self, level, template, **props):
        line = f"{datetime.datetime.now(datetime.timezone.utc).isoformat(timespec='milliseconds')} {level} " + template.format(**props)
        print(line, flush=True)
        if not self.url:
            return
        ev = {"@t": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z"),
              "@mt": template, "Service": "py-portal", "Node": NODE, **props}
        if level != "Information":
            ev["@l"] = level
        try:
            self.q.put_nowait(ev)
        except queue.Full:
            pass

    def _pump(self):
        while True:
            batch = [self.q.get()]
            time.sleep(1)
            while not self.q.empty() and len(batch) < 500:
                batch.append(self.q.get_nowait())
            body = "\n".join(json.dumps(e) for e in batch).encode()
            req = urllib.request.Request(self.url, data=body, method="POST", headers={
                "X-Seq-ApiKey": self.key, "Content-Type": "application/vnd.serilog.clef"})
            for attempt in range(3):
                try:
                    urllib.request.urlopen(req, timeout=10).read()
                    break
                except OSError as e:
                    print(f"log ingest failed ({attempt + 1}/3): {e}", flush=True)
                    time.sleep(2 * (attempt + 1))


LOG = SeqLog(FORGE_URL, FORGE_LOG_KEY)


# ------------------------------------------------------------------ database
# Database time spent by the request on this thread (one request per thread).
REQUEST = threading.local()


def timed_query(name, fn):
    """Runs fn(), logs its time as one Information event and adds it to the
    request's database total."""
    start = time.monotonic()
    try:
        result = fn()
    finally:
        ms = (time.monotonic() - start) * 1000
        REQUEST.db_ms = getattr(REQUEST, "db_ms", 0.0) + ms
    rows = len(result) if isinstance(result, list) else 1
    LOG.event("Information", "Database {Query} took {Elapsed} ms ({Rows} rows)",
              Query=name, Elapsed=round(ms, 2), Rows=rows)
    return result


class Db:
    def __init__(self, url):
        self.url = url
        self.ready = False

    @property
    def configured(self):
        return bool(self.url) and psycopg is not None

    def status(self):
        if not self.url:
            return "not configured (set DATABASE_URL)"
        if psycopg is None:
            return "psycopg is not installed"
        return "ok" if self.ping() else "unreachable"

    def _connect(self):
        conn = psycopg.connect(self.url, connect_timeout=3, autocommit=True)
        if not self.ready:
            conn.execute("""CREATE TABLE IF NOT EXISTS notes (
                id BIGSERIAL PRIMARY KEY, app TEXT NOT NULL, node TEXT NOT NULL,
                body TEXT NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT now())""")
            self.ready = True
        return conn

    def ping(self):
        def run():
            with self._connect() as c:
                return c.execute("SELECT 1").fetchone()
        try:
            timed_query("ping", run)
            return True
        except Exception as e:  # noqa: BLE001 - report any driver error as down
            LOG.event("Warning", "Database ping failed: {Error}", Error=str(e))
            return False

    def notes(self, limit=50):
        def run():
            with self._connect() as c:
                return c.execute("SELECT id, app, node, body, created_at FROM notes ORDER BY id DESC LIMIT %s", (limit,)).fetchall()
        return timed_query("list notes", run)

    def add(self, body):
        def run():
            with self._connect() as c:
                return c.execute("INSERT INTO notes (app, node, body) VALUES ('python', %s, %s) RETURNING id",
                                 (NODE, body)).fetchone()[0]
        return timed_query("insert note", run)


DB = Db(DATABASE_URL)


def addresses():
    try:
        out = subprocess.run(["hostname", "-I"], capture_output=True, text=True, timeout=2).stdout
        return out.split()
    except OSError:
        return [socket.gethostbyname(NODE)]


# ------------------------------------------------------------------ pages
STYLE = """body{font:15px system-ui;margin:0;background:#0d1b2a;color:#e0e8f0;display:flex;flex-direction:column;min-height:100vh}
nav{display:flex;gap:1rem;padding:.8rem 2rem;background:#122438;border-bottom:1px solid #253a52}nav a{color:#8aa4bf;text-decoration:none}
nav a.on{color:#5bc0eb;font-weight:600}main{flex:1;padding:1.5rem 2rem}h1{color:#5bc0eb;margin-top:0}
table{border-collapse:collapse}th,td{padding:.35rem .8rem;border-bottom:1px solid #253a52;text-align:left}th{color:#8aa4bf;font-weight:500}
input,button{font:inherit;padding:.4rem .6rem;border-radius:4px;border:1px solid #253a52;background:#0d1b2a;color:inherit}button{background:#5bc0eb;color:#0d1b2a;border:0;cursor:pointer}
footer{padding:.7rem 2rem;background:#122438;border-top:1px solid #253a52;font-size:13px;color:#8aa4bf}footer b{color:#5bc0eb}.err{color:#f87171}"""
PAGES = [("/", "Home"), ("/notes", "Notes"), ("/health", "Health"), ("/about", "About")]


def page(path, title, body, hits):
    nav = "".join(f'<a href="{p}" class="{"on" if p == path else ""}">{t}</a>' for p, t in PAGES)
    return f"""<!doctype html><html><head><meta charset="utf-8"><title>Python portal · {title} · {html.escape(NODE)}</title>
<meta name="forge-node" content="{html.escape(NODE)}"><meta name="viewport" content="width=device-width"><style>{STYLE}</style></head>
<body><nav><b>Python portal</b>{nav}</nav><main>{body}</main>
<footer>Served by node <b>{html.escape(NODE)}</b> · {RUNTIME} · pid {os.getpid()} · request #{hits}</footer></body></html>"""


def table(rows):
    return "<table>" + "".join(f"<tr><th>{html.escape(str(k))}</th><td>{html.escape(str(v))}</td></tr>" for k, v in rows) + "</table>"


class Handler(http.server.BaseHTTPRequestHandler):
    def info(self, hits):
        return {
            "portal": "python", "runtime": RUNTIME, "node": NODE, "addresses": addresses(), "port": PORT,
            "pid": os.getpid(), "hits": hits, "host_header": self.headers.get("Host", ""),
            "client": self.client_address[0], "x_forwarded_for": self.headers.get("X-Forwarded-For", ""),
            "x_real_ip": self.headers.get("X-Real-IP", ""), "scheme": self.headers.get("X-Forwarded-Proto", "http"),
            "time": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
        }

    def hit(self):
        global HITS
        with HITS_LOCK:
            HITS += 1
            return HITS

    def send(self, status, body, ctype="text/html; charset=utf-8", extra=None):
        data = body.encode() if isinstance(body, str) else body
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("X-Served-By", NODE)
        for k, v in (extra or {}).items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(data)
        return status

    def do_GET(self):
        REQUEST.db_ms = 0.0
        start, hits = time.monotonic(), self.hit()
        url = urllib.parse.urlsplit(self.path)
        q = urllib.parse.parse_qs(url.query)
        try:
            status = self.route_get(url.path, q, hits)
        except Exception as e:  # noqa: BLE001 - show and log any page error
            LOG.event("Error", "Page {Path} failed: {Error}", Path=url.path, Error=str(e))
            status = self.send(500, page(url.path, "Error", f'<h1>Error</h1><p class="err">{html.escape(str(e))}</p>', hits))
        self.log_request_event("GET", url.path, status, start)

    def route_get(self, path, q, hits):
        info = self.info(hits)
        if path == "/":
            if q.get("format") == ["json"]:
                return self.send(200, json.dumps(info), "application/json")
            return self.send(200, page(path, "Home", f"""<h1>Hello from {html.escape(NODE)}</h1>
<p>This page was served by node <b>{html.escape(NODE)}</b> ({html.escape(', '.join(info['addresses']))}). Reload it behind a load
balancer and watch the node name in the footer change.</p><p>Database: <b>{html.escape(DB.status())}</b></p>""", hits))
        if path == "/notes":
            if not DB.configured:
                return self.send(200, page(path, "Notes", f"<h1>Notes</h1><p>Database {html.escape(DB.status())}.</p>", hits))
            rows = "".join(f"<tr><td>{n[0]}</td><td>{html.escape(n[1])}</td><td>{html.escape(n[2])}</td><td>{html.escape(n[3])}</td>"
                           f"<td>{n[4]:%Y-%m-%d %H:%M:%S}</td></tr>" for n in DB.notes())
            return self.send(200, page(path, "Notes", f"""<h1>Notes</h1>
<form method="post" action="/notes"><input name="body" placeholder="Write a note" required maxlength="500"> <button>Add</button></form>
<p>Each note records the app and node that saved it.</p>
<table><tr><th>#</th><th>App</th><th>Node</th><th>Note</th><th>Created (UTC)</th></tr>{rows}</table>""", hits))
        if path == "/health":
            db = DB.status()
            ok = db in ("ok",) or not DATABASE_URL
            return self.send(200 if ok else 503, json.dumps({"status": "ok" if ok else "degraded", "node": NODE, "database": db}),
                             "application/json")
        if path == "/about":
            return self.send(200, page(path, "About", "<h1>About this node</h1>" + table(
                (k, ", ".join(v) if isinstance(v, list) else v) for k, v in info.items()), hits))
        return self.send(404, page(path, "Not found", "<h1>Not found</h1>", hits))

    def do_POST(self):
        REQUEST.db_ms = 0.0
        start, hits = time.monotonic(), self.hit()
        path = urllib.parse.urlsplit(self.path).path
        if path != "/notes" or not DB.configured:
            status = self.send(404, page(path, "Not found", "<h1>Not found</h1>", hits))
        else:
            n = min(int(self.headers.get("Content-Length", "0") or 0), 4096)
            body = urllib.parse.parse_qs(self.rfile.read(n).decode()).get("body", [""])[0].strip()[:500]
            if body:
                note_id = DB.add(body)
                LOG.event("Information", "Note {NoteId} added on {Node}", NoteId=note_id, Node=NODE)
            status = self.send(303, "", extra={"Location": "/notes"})
        self.log_request_event("POST", path, status, start)

    def log_request_event(self, method, path, status, start):
        level = "Error" if status >= 500 else "Warning" if status >= 400 else "Information"
        # Same shape as Serilog's request log in the .NET portal.
        LOG.event(level, "HTTP {RequestMethod} {RequestPath} responded {StatusCode} in {Elapsed} ms (database {DbMs} ms)",
                  RequestMethod=method, RequestPath=path, StatusCode=status,
                  Elapsed=round((time.monotonic() - start) * 1000, 1), DbMs=round(getattr(REQUEST, "db_ms", 0.0), 1),
                  Client=self.headers.get("X-Real-IP") or self.client_address[0])

    def log_message(self, fmt, *args):
        pass  # requests are logged by log_request_event


if __name__ == "__main__":
    LOG.event("Information", "Python portal starting on {Node}:{Port}; database {Database}; logs to Forge {Logs}",
              Node=NODE, Port=PORT, Database=DB.status(), Logs="on" if LOG.url else "off")
    http.server.ThreadingHTTPServer(("0.0.0.0", PORT), Handler).serve_forever()
