#!/usr/bin/env python3
"""Build the benchmark report (../report.html) from data.json.

Usage: python3 -I build_report.py
Standard library only. The page is self-contained: inline SVG charts, inline
CSS and a small inline script for tooltips, so it opens offline.
"""
import html
import json
import pathlib

HERE = pathlib.Path(__file__).resolve().parent
D = json.loads((HERE / "data.json").read_text())
OUT = HERE.parent / "report.html"

# Entity -> colour slot (fixed; colour follows the entity in every chart).
SLOT = {"server": 1, "worker": 2, "engine": 3, "agent": 4, "helper": 5, "stored": 6}
NAME = {"server": "forge-server", "worker": "worker", "engine": "chDB (own memory)",
        "agent": "forge-agent", "helper": "agent helper", "stored": "stored / acked"}

W, H = 640, 260
ML, MR, MT, MB = 56, 120, 16, 36  # right margin leaves room for direct labels
PW, PH = W - ML - MR, H - MT - MB


def esc(s):
    return html.escape(str(s), quote=True)


def fmt(v, unit=""):
    if v is None:
        return "–"
    if abs(v) >= 1000:
        s = f"{v:,.0f}"
    elif abs(v) >= 100:
        s = f"{v:.0f}"
    else:
        s = f"{v:.1f}".rstrip("0").rstrip(".")
    return s + unit


def nice_max(v):
    if v <= 0:
        return 1
    for m in (1, 2, 2.5, 5, 10):
        for e in range(-1, 8):
            c = m * 10 ** e
            if c >= v:
                return c
    return v


def ticks(vmax, n=4):
    return [vmax * i / n for i in range(n + 1)]


def axes(ymax, unit, xlabels):
    out = []
    for t in ticks(ymax):
        y = MT + PH - t / ymax * PH
        out.append(f'<line class="grid" x1="{ML}" x2="{ML + PW}" y1="{y:.1f}" y2="{y:.1f}"/>')
        out.append(f'<text class="tick" x="{ML - 8}" y="{y + 4:.1f}" text-anchor="end">{esc(fmt(t))}</text>')
    out.append(f'<line class="axis" x1="{ML}" x2="{ML + PW}" y1="{MT + PH}" y2="{MT + PH}"/>')
    for x, label in xlabels:
        out.append(f'<text class="tick" x="{x:.1f}" y="{MT + PH + 18}" text-anchor="middle">{esc(label)}</text>')
    out.append(f'<text class="unit" x="{ML - 8}" y="{MT - 4}" text-anchor="end">{esc(unit)}</text>')
    return out


def place_labels(ends):
    """Nudge end-of-line labels apart so they never overlap (min 14px)."""
    ends = sorted(ends, key=lambda e: e[0])
    for i in range(1, len(ends)):
        if ends[i][0] - ends[i - 1][0] < 14:
            ends[i] = (ends[i - 1][0] + 14,) + ends[i][1:]
    overflow = ends[-1][0] - (MT + PH) if ends else 0
    if overflow > 0:  # keep the stack inside the plot: shift it up as one block
        ends = [(e[0] - overflow,) + e[1:] for e in ends]
    return ends


def line_chart(title, sub, series, unit, xmax=None, xlabel="s", band=None, ymax=None, cats=None):
    """series: [(entity_key, label, [[x, y], ...])]. cats: categorical x labels."""
    allx = [p[0] for _, _, pts in series for p in pts]
    ally = [p[1] for _, _, pts in series for p in pts]
    if not ally:
        return ""
    ymax = ymax or nice_max(max(ally) * 1.05)
    if cats:
        n = len(cats)
        xs = lambda i: ML + (PW * i / (n - 1) if n > 1 else PW / 2)
        xl = [(xs(i), c) for i, c in enumerate(cats)]
    else:
        xmax = xmax or max(allx)
        xs = lambda x: ML + x / xmax * PW
        step = nice_max(xmax / 6)
        xl = [(xs(t), f"{t:.0f}{xlabel}") for t in [step * i for i in range(int(xmax // step) + 1)]]
    ys = lambda y: MT + PH - min(y, ymax) / ymax * PH
    out = axes(ymax, unit, xl)
    if band:
        x0, x1, label = band
        out.insert(0, f'<rect class="band" x="{xs(x0):.1f}" y="{MT}" width="{xs(x1) - xs(x0):.1f}" height="{PH}"/>'
                      f'<text class="bandlabel" x="{xs(x0) + 6:.1f}" y="{MT + 14}">{esc(label)}</text>')
    ends, hits = [], []
    for key, label, pts in series:
        if not pts:
            continue
        cls = f"s{SLOT[key]}"
        path = " ".join(f"{'M' if i == 0 else 'L'}{xs(x):.1f},{ys(y):.1f}" for i, (x, y) in enumerate(pts))
        out.append(f'<path class="line {cls}" d="{path}"/>')
        for x, y in pts:
            tip = f"{label}: {fmt(y, ' ' + unit)}" + (f" · {cats[x]}" if cats else f" · t={x:.0f}s")
            hits.append(f'<circle class="hit {cls}" cx="{xs(x):.1f}" cy="{ys(y):.1f}" r="9" data-tip="{esc(tip)}"/>')
        lx, ly = pts[-1]
        ends.append((ys(ly), label, cls))
    for y, label, cls in place_labels(ends):
        out.append(f'<text class="dlabel" x="{ML + PW + 8}" y="{y + 4:.1f}">'
                   f'<tspan class="key {cls}">●</tspan> {esc(label)}</text>')
    return figure(title, sub, out + hits, series, unit, cats)


def bar_chart(title, sub, cats, groups, unit, threshold=None, ymax=None):
    """groups: [(entity_key, label, [value per cat])]."""
    vals = [v for _, _, vs in groups for v in vs if v is not None]
    ymax = ymax or nice_max(max(vals + [threshold or 0]) * 1.08)
    n, g = len(cats), len(groups)
    slot = PW / n
    bw = min(28, (slot * 0.7 - 2 * (g - 1)) / g)
    xl = [(ML + slot * (i + 0.5), c) for i, c in enumerate(cats)]
    out = axes(ymax, unit, xl)
    ys = lambda y: MT + PH - min(y, ymax) / ymax * PH
    for gi, (key, label, vs) in enumerate(groups):
        cls = f"s{SLOT[key]}"
        for i, v in enumerate(vs):
            if v is None:
                continue
            x = ML + slot * (i + 0.5) - (g * bw + 2 * (g - 1)) / 2 + gi * (bw + 2)
            y = ys(v)
            h = MT + PH - y
            r = min(4, h / 2, bw / 2)
            # rounded top only, anchored to the baseline
            d = (f"M{x:.1f},{MT + PH} L{x:.1f},{y + r:.1f} Q{x:.1f},{y:.1f} {x + r:.1f},{y:.1f} "
                 f"L{x + bw - r:.1f},{y:.1f} Q{x + bw:.1f},{y:.1f} {x + bw:.1f},{y + r:.1f} L{x + bw:.1f},{MT + PH} Z")
            tip = f"{label} · {cats[i]}: {fmt(v, ' ' + unit)}"
            out.append(f'<path class="bar {cls}" d="{d}" data-tip="{esc(tip)}"/>')
            if v > ymax:
                out.append(f'<text class="over" x="{x + bw / 2:.1f}" y="{MT + 10}" text-anchor="middle">↑{esc(fmt(v))}</text>')
    if threshold:
        y = ys(threshold)
        out.append(f'<line class="threshold" x1="{ML}" x2="{ML + PW}" y1="{y:.1f}" y2="{y:.1f}"/>'
                   f'<text class="thlabel" x="{ML + PW + 8}" y="{y + 4:.1f}">limit {esc(fmt(threshold, " " + unit))}</text>')
    if g > 1:
        legend_y = MT + 8
        for gi, (key, label, _) in enumerate(groups):
            out.append(f'<text class="dlabel" x="{ML + PW + 8}" y="{legend_y + 34 + gi * 16}">'
                       f'<tspan class="key s{SLOT[key]}">■</tspan> {esc(label)}</text>')
    table_series = [(k, l, [[i, v] for i, v in enumerate(vs)]) for k, l, vs in groups]
    return figure(title, sub, out, table_series, unit, cats)


def figure(title, sub, svg_parts, series, unit, cats=None):
    rows = {}
    for _, label, pts in series:
        for x, y in pts:
            rows.setdefault(x, {})[label] = y
    labels = [l for _, l, _ in series]
    head = "".join(f"<th>{esc(l)}</th>" for l in labels)
    body = "".join(
        f"<tr><td>{esc(cats[x] if cats else f'{x:.0f} s')}</td>"
        + "".join(f"<td>{esc(fmt(rows[x].get(l)))}</td>" for l in labels) + "</tr>"
        for x in sorted(rows))
    return (f'<figure class="chart"><figcaption><h3>{esc(title)}</h3><p>{sub}</p></figcaption>'
            f'<svg viewBox="0 0 {W} {H}" role="img" aria-label="{esc(title)}">{"".join(svg_parts)}</svg>'
            f'<details><summary>Data table ({esc(unit)})</summary><div class="tw"><table><thead><tr>'
            f'<th>{"Phase" if cats else "Time"}</th>{head}</tr></thead><tbody>{body}</tbody></table></div></details></figure>')


def ts(block, key):
    return (block or {}).get("series", {}).get(key, [])


def scale(points, f):
    return [[x, round(y * f, 2)] for x, y in points]


def tile(value, label, hint=""):
    return f'<div class="tile"><div class="v">{value}</div><div class="l">{label}</div><div class="h">{hint}</div></div>'


# ---------- the numbers the text quotes (all from data.json) ----------
cap, duck = D["capped"], D["duck"]
u50, a50 = D["uncapped50"], D["agent50"]
byid = {p["id"]: p for p in duck}
dl, d50, d100, d100x20 = (byid.get(k) or {} for k in ("d_light", "d_p50", "d_p100", "d_p100x20"))
d100t, dnr = byid.get("d_p100_tuned") or {}, byid.get("d_p100x20nr") or {}
c500 = next((p for p in cap if p["id"] == "p500c"), {})
c1000 = next((p for p in cap if p["id"] == "p1000"), {})
ats = D["agent_ts"]
chdb_own_max = max((p[1] for p in ts(D["uncapped50_ts"], "engine_anon_mb")), default=0)


def replay_peak(wid, store):
    """Peak store memory over the whole run, drain included (MB)."""
    for w in D.get("replay", []):
        r = w["stores"].get(store)
        if w["id"] == wid and r:
            return max(p[2] for p in r["points"])
    return None


duck50_mem = replay_peak("p50", "duck") or d50.get("worker_rss")
chdb_flow_mem = (replay_peak("p50", "chdb") or ((u50["worker_rss"] or 0) + chdb_own_max)) + 520
chdb_flow_cpu = ((u50["worker_cpu"] or 0) + (u50["engine_cpu"] or 0)) / 100
agent_cpu = [p[1] for p in ts(ats, "a:agent_cpu_core_pct")]
agent_mem = [p[1] for p in ts(ats, "a:agent_VmRSS_mb")]
helper_mem = [p[1] for p in ts(ats, "a:helper_VmRSS_mb")]


def pct(a, b):
    return f"{a / b * 100:.0f} %" if a and b else "–"


def cores(v):
    return fmt((v or 0) / 100)


def band(block):
    t = (block or {}).get("t_ready") or 0
    return (t, t + 240, "load running")


xmax_50 = max(max((p[0] for p in ts(D["uncapped50_ts"], "server_rss_mb")), default=1),
              max((p[0] for p in ts(D["duck50_ts"], "server_rss_mb")), default=1))
cpu_max_50 = nice_max(max([p[1] for k in ("server_cpu_core_pct", "worker_cpu_core_pct", "engine_cpu_core_pct")
                           for b in (D["uncapped50_ts"], D["duck50_ts"]) for p in ts(b, k)] or [1]) / 100 * 1.05)
mem_max_50 = nice_max(max([p[1] for k in ("server_rss_mb", "worker_rss_mb", "engine_anon_mb")
                           for b in (D["uncapped50_ts"], D["duck50_ts"]) for p in ts(b, k)] or [1]) * 1.05)

# ---------- charts ----------
C = {}
C["cmp_cpu"] = bar_chart(
    "CPU used by the log store",
    "Taller bar = more CPU. 1.0 means one full CPU core busy all the time.",
    ["chDB", "DuckDB"], [("engine", "CPU", [chdb_flow_cpu, (d50.get("worker_cpu") or 0) / 100])], "cores")
C["cmp_mem"] = bar_chart(
    "Memory used by the log store",
    "Highest point of the whole run, including the minute after the load when the last waiting logs are saved. chDB's bar includes its 520 MB program file.",
    ["chDB", "DuckDB"], [("worker", "memory", [chdb_flow_mem, duck50_mem or 0])], "MB")
C["cpu_chdb"] = line_chart(
    "chDB: CPU over time",
    "The grey area is when logs were being sent. Lines are each program's CPU, in cores.",
    [("server", "forge-server", scale(ts(D["uncapped50_ts"], "server_cpu_core_pct"), 0.01)),
     ("worker", "worker", scale(ts(D["uncapped50_ts"], "worker_cpu_core_pct"), 0.01)),
     ("engine", "chDB", scale(ts(D["uncapped50_ts"], "engine_cpu_core_pct"), 0.01))],
    "cores", xmax=xmax_50, band=band(D["uncapped50_ts"]), ymax=cpu_max_50)
C["cpu_duck"] = line_chart(
    "DuckDB: CPU over time",
    "Same load, same scale. DuckDB lives inside the worker, so the orange line is the whole store.",
    [("server", "forge-server", scale(ts(D["duck50_ts"], "server_cpu_core_pct"), 0.01)),
     ("worker", "worker + DuckDB", scale(ts(D["duck50_ts"], "worker_cpu_core_pct"), 0.01))],
    "cores", xmax=xmax_50, band=band(D["duck50_ts"]), ymax=cpu_max_50)
C["mem_chdb"] = line_chart(
    "chDB: memory over time",
    "chDB's own memory. Its 520 MB program file comes on top and is not drawn.",
    [("server", "forge-server", ts(D["uncapped50_ts"], "server_rss_mb")),
     ("worker", "worker", ts(D["uncapped50_ts"], "worker_rss_mb")),
     ("engine", "chDB", ts(D["uncapped50_ts"], "engine_anon_mb"))],
    "MB", xmax=xmax_50, band=band(D["uncapped50_ts"]), ymax=mem_max_50)
C["mem_duck"] = line_chart(
    "DuckDB: memory over time",
    "Same load, same scale. Nothing else to add: there is no separate engine.",
    [("server", "forge-server", ts(D["duck50_ts"], "server_rss_mb")),
     ("worker", "worker + DuckDB", ts(D["duck50_ts"], "worker_rss_mb"))],
    "MB", xmax=xmax_50, band=band(D["duck50_ts"]), ymax=mem_max_50)
C["duck_ingest"] = line_chart(
    "DuckDB: logs sent vs logs saved, per test",
    "When the two lines sit on top of each other, DuckDB keeps up. When the green line drops below, logs start to queue.",
    [("server", "sent", [[i, p["offered"]] for i, p in enumerate(duck)]),
     ("stored", "saved", [[i, p["acked"]] for i, p in enumerate(duck)])],
    "lines/s", cats=[p["label"] for p in duck])
C["duck_search"] = bar_chart(
    "DuckDB: how long a search takes (slowest 5 %)",
    "Bars above the red line mean the Logs page feels slow (over 5 seconds).",
    [p["label"] for p in duck],
    [("engine", "log search", [p["search_p95"] and p["search_p95"] / 1000 for p in duck]),
     ("agent", "log chart (histogram)", [p["hist_p95"] and p["hist_p95"] / 1000 for p in duck])],
    "s", threshold=5)
C["scale_ingest"] = line_chart(
    "chDB (2 cores): logs sent vs logs saved as servers are added",
    "Same as above: lines apart = chDB is behind and logs wait on the servers' disks.",
    [("server", "sent", [[i, p["offered"]] for i, p in enumerate(cap)]),
     ("stored", "saved", [[i, p["acked"]] for i, p in enumerate(cap)])],
    "lines/s", cats=[p["label"] + " servers" for p in cap])
C["scale_search"] = bar_chart(
    "chDB (2 cores): how long a search takes (slowest 5 %)",
    "Bars above the red line mean the Logs page feels slow.",
    [p["label"] for p in cap], [("engine", "log search", [p["search_p95"] and p["search_p95"] / 1000 for p in cap])],
    "s", threshold=5)
C["agent_cpu"] = line_chart(
    "forge-agent on a real server: CPU",
    "% of ONE core (100 % = one core fully busy). Short spikes are the agent packing and sending a log file.",
    [("agent", "forge-agent", ts(ats, "a:agent_cpu_core_pct")),
     ("helper", "agent helper", ts(ats, "a:helper_cpu_core_pct"))],
    "% core", band=band(ats))
C["agent_mem"] = line_chart(
    "forge-agent on a real server: memory",
    "Memory rises in the first minute, then stays flat. Flat = no leak.",
    [("agent", "forge-agent", ts(ats, "a:agent_VmRSS_mb")),
     ("helper", "agent helper", ts(ats, "a:helper_VmRSS_mb"))],
    "MB", band=band(ats))


# ---------- page pieces ----------
def verdict(kind, text):
    icon = {"good": "✓", "warn": "!", "bad": "✕"}[kind]
    word = {"good": "Good", "warn": "Watch", "bad": "Limit"}[kind]
    return f'<div class="verdict {kind}"><span class="vi" aria-hidden="true">{icon}</span><b>{word}:</b> {text}</div>'


def section(n, question, setup, body, findings, verdicts):
    li = "".join(f"<li>{f}</li>" for f in findings)
    return (f'<section class="test"><div class="qn">Test {n}</div><h2>{question}</h2>'
            f'<p class="setup"><b>How we tested:</b> {setup}</p>{body}'
            f'<div class="find"><h4>What this shows</h4><ul>{li}</ul></div>{"".join(verdicts)}</section>')


def g2(a, b=""):
    return f'<div class="grid2">{a}{b}</div>'


FLOW = """<svg viewBox="0 0 900 230" role="img" aria-label="How logs flow through Forge" class="flow">
<defs><marker id="ar" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto"><path d="M0,0 L10,5 L0,10 z" class="arh"/></marker></defs>
<g class="box"><rect x="10" y="20" width="150" height="70" rx="10"/><text x="85" y="50">Your apps</text><text x="85" y="70" class="sm">containers &amp; services</text></g>
<g class="box hi"><rect x="220" y="20" width="170" height="70" rx="10"/><text x="305" y="50">forge-agent</text><text x="305" y="70" class="sm">one per server</text></g>
<g class="box"><rect x="220" y="140" width="170" height="70" rx="10"/><text x="305" y="170">spool on disk</text><text x="305" y="190" class="sm">logs wait here if needed</text></g>
<g class="box hi"><rect x="450" y="20" width="190" height="70" rx="10"/><text x="545" y="50">analytics worker</text><text x="545" y="70" class="sm">receives &amp; saves logs</text></g>
<g class="box"><rect x="700" y="20" width="190" height="70" rx="10"/><text x="795" y="50">log store</text><text x="795" y="70" class="sm">chDB or DuckDB</text></g>
<g class="box"><rect x="450" y="140" width="190" height="70" rx="10"/><text x="545" y="170">forge-server</text><text x="545" y="190" class="sm">console, watches worker</text></g>
<path d="M160,55 L218,55" class="arw" marker-end="url(#ar)"/>
<path d="M290,90 L290,138" class="arw" marker-end="url(#ar)"/><path d="M320,138 L320,92" class="arw" marker-end="url(#ar)"/>
<path d="M390,55 L448,55" class="arw" marker-end="url(#ar)"/><text x="419" y="45" class="sm">upload</text>
<path d="M640,55 L698,55" class="arw" marker-end="url(#ar)"/>
<path d="M545,138 L545,92" class="arw dash" marker-end="url(#ar)"/><text x="555" y="120" class="sm">search</text>
</svg>"""

GLOSS = [
    ("Server (node)", "One machine running forge-agent. A lightweight install has 2–5."),
    ("Service", "An app on a server (a container or systemd service). A server can run 20–40."),
    ("Lines/s", "Log lines written per second. A quiet app writes well under 1; our tests used 10–40 per app to stress the system."),
    ("Sent vs saved", "Sent = what the apps produced. Saved = what reached the store. Equal means the store keeps up."),
    ("Spool / backlog", "If the store is busy, the agent keeps logs in files on its own disk and sends them later. Nothing is lost, it just arrives late."),
    ("Drain time", "After the load stops, how long until every waiting log is saved."),
    ("p95", "95 % of searches finish faster than this. It describes the slow end, the searches people notice."),
    ("1 core / 100 %", "One CPU core fully busy. 0.5 core = half of one core. A 4-core VPS has 4."),
    ("Memory (RSS)", "RAM the program holds. chDB also loads a 520 MB program file; the OS can share and reclaim it, but it shows up in RSS."),
    ("Cap", "A limit we set on purpose (e.g. chDB at 2 cores / 2 GB) to see what happens on a small box."),
]
gloss = "".join(f"<dt>{esc(a)}</dt><dd>{esc(b)}</dd>" for a, b in GLOSS)

SIZING = [
    ("Lightweight", "2–5 servers, a few apps logging now and then", "DuckDB",
     "under 100 MB; 0 % CPU idle, about 0.005 core to save logs (searches add a little while you use them)", "good"),
    ("Small", "up to 50 servers, busy logging (~5,000 lines/s)", "DuckDB",
     f"about {fmt(duck50_mem, ' MB')} and {cores(d50.get('worker_cpu'))} cores; searches under 1 s", "good"),
    ("Medium", "~100 servers, busy logging (~10,000 lines/s)", "DuckDB at its edge, or chDB",
     f"DuckDB keeps up but charts get slow ({fmt((d100.get('hist_p95') or 0) / 1000, ' s')}); chDB uses more RAM", "warn"),
    ("Large", "500+ servers", "chDB (≥ 2 cores per 500 servers) or external ClickHouse",
     f"chDB on 2 cores saved about {fmt(c500.get('acked'))} lines/s at 500 servers", "warn"),
]
sizing = "".join(
    f'<tr><td><b>{esc(a)}</b></td><td>{esc(b)}</td><td>{esc(c)}</td><td><span class="pill {k}">{esc(d)}</span></td></tr>'
    for a, b, c, d, k in SIZING)

CMP = [
    ("Memory at 50 busy servers", fmt(chdb_flow_mem, " MB"), fmt(duck50_mem, " MB"), "DuckDB"),
    ("CPU at 50 busy servers", fmt(chdb_flow_cpu) + " cores", cores(d50.get("worker_cpu")) + " cores", "about equal"),
    ("Idle cost (no logs)", "13–16 % of a core, ~100 MB + 520 MB file", "0 % CPU, 44 MB", "DuckDB"),
    ("Search at 50 busy servers (p95)", fmt((u50.get("search_p95") or 0) / 1000, " s"), fmt((d50.get("search_p95") or 0) / 1000, " s"), "chDB, slightly"),
    ("Most it can take on this host", "far more with more cores (10k lines/s on just 2)", f"about {fmt(d100.get('acked'))} lines/s", "chDB"),
    ("Extra program to run", "yes, a separate engine process", "no, built into the worker", "DuckDB"),
    ("Lost logs in any test", "0", "0", "both"),
]
cmp_rows = "".join(f"<tr><td>{esc(a)}</td><td>{esc(b)}</td><td>{esc(c)}</td><td><b>{esc(d)}</b></td></tr>" for a, b, c, d in CMP)

runs = [
    ("2026-11-20", "Before the redesign: logs went through forge-server", "2026-11-20-sqlite-chdb-before-worker.md"),
    ("2026-11-21", "Seq in Docker, for comparison", "2026-11-21-seq-docker.md"),
    ("2026-11-21", "chDB on 2 cores, 50 → 2,000 servers", "2026-11-21-chdb-capped-node-scaling.md"),
    ("2026-11-22", "chDB, no limits, 50 servers × 20 services", "2026-11-22-chdb-uncapped-50x20.md"),
    ("2026-11-22", "chDB, 50 servers with one real agent", "2026-11-22-chdb-real-agent.md"),
    ("2026-11-22", "DuckDB against chDB", "2026-11-22-duckdb-vs-chdb.md"),
]
run_list = "".join(f'<li><span class="d">{d}</span> <a href="{f}">{esc(t)}</a></li>' for d, t, f in runs)

tests = [
    section(1, "Which log store is lighter: chDB or DuckDB?",
            "50 servers, each running 20 services; 10 of them write 10 log lines a second (5,000 lines/s in total). Same load on each store, nothing capped.",
            g2(C["cmp_cpu"], C["cmp_mem"]) + g2(C["cpu_chdb"], C["cpu_duck"]) + g2(C["mem_chdb"], C["mem_duck"]),
            [f"<b>Memory:</b> DuckDB needs about {fmt(duck50_mem, ' MB')}; chDB plus the worker about {fmt(chdb_flow_mem, ' MB')}, most of it chDB's program file.",
             f"<b>CPU:</b> about the same while busy ({cores(d50.get('worker_cpu'))} vs {fmt(chdb_flow_cpu)} cores).",
             "<b>chDB's CPU keeps climbing</b> during the run even though the load is steady: it is busy merging many small writes in the background.",
             "<b>forge-server</b> stays small and calm on both (under 0.4 core, under 100 MB). It no longer touches the logs."],
            [verdict("good", "DuckDB does the same job with about a third of the memory and no extra program. Right default for small installs.")]),
    section(2, "How far can DuckDB go before it falls behind?",
            "We kept adding load: 5 servers lightly, then 50 and 100 servers, then each logging app at 20 lines/s.",
            g2(C["duck_ingest"], C["duck_search"]),
            [f"<b>5 and 50 servers:</b> every line saved as it arrives, searches fast ({fmt((d50.get('search_p95') or 0) / 1000, ' s')} at 50).",
             f"<b>100 servers at 10/s:</b> still saves {pct(d100.get('acked'), d100.get('offered'))} as it arrives, but the log chart takes up to {fmt((d100.get('hist_p95') or 0) / 1000, ' s')}. <b>After tuning</b>, the queue empties {fmt(d100t.get('drain'), ' s')} after the load (was {fmt(d100.get('drain'), ' s')}), the store uses {cores(d100t.get('worker_cpu'))} cores (was {cores(d100.get('worker_cpu'))}) and the chart takes {fmt((d100t.get('hist_p95') or 0) / 1000, ' s')}.",
             f"<b>100 servers at 20/s:</b> saves only {pct(d100x20.get('acked'), d100x20.get('offered'))} as it arrives; the rest queues on the servers and arrives later. Searches pass 5 s.",
             f"<b>Why:</b> searches cost far more than saving. With nobody searching, DuckDB saved {fmt(dnr.get('acked'))} lines/s on just {cores(dnr.get('worker_cpu'))} cores. Our test had 4 people searching every 2 seconds non-stop, much more than real use. DuckDB has no text index, so a search reads every line in its time window.",
             "<b>Tuning done:</b> searches are now limited so they cannot starve saving, and saves no longer wait to batch."],
            [verdict("good", "Comfortable up to about 50 busy servers, far beyond a 2–5 server install."),
             verdict("warn", "About 100 busy servers is the pressure point on this host: logs arrive late and the Logs page gets slow.")]),
    section(3, "What happens with chDB on a small box (2 cores, 2 GB) as servers are added?",
            "Each server writes 20 lines/s. We went from 50 to 2,000 servers, with chDB limited to 2 CPU cores and 2 GB of RAM.",
            g2(C["scale_ingest"], C["scale_search"]),
            [f"<b>Up to 500 servers:</b> keeps up (about {fmt(c500.get('acked'))} lines/s saved).",
             f"<b>1,000 servers and up:</b> falls behind (saves about {fmt(c1000.get('acked'))} of {fmt(c1000.get('offered'))} lines/s); the rest waits on the servers and drains later.",
             "<b>Searches</b> slow down as chDB's 2 cores fill up. Above ~1,400 servers they failed entirely, until a bug was fixed (the request no longer fits in a URL).",
             "<b>Nothing was lost</b>, even at 2,000 servers. A bug that saved some logs twice was found and fixed."],
            [verdict("good", "chDB scales further than DuckDB when given cores; it is the choice for big fleets."),
             verdict("warn", "Its idle cost (CPU and the 520 MB file) is wasted on a small install.")]),
    section(4, "How heavy is the agent on each server?",
            "A real forge-agent on this machine collected logs from 20 Docker containers (10 logging, 100 lines/s). We also cut its link to the worker for 60 s.",
            g2(C["agent_cpu"], C["agent_mem"]),
            [f"<b>CPU:</b> about 0.6 % of one core on average, short spikes to {fmt(max(agent_cpu, default=0))} % when it sends a file.",
             f"<b>Memory:</b> {fmt(max(agent_mem, default=0), ' MB')} for the agent and {fmt(max(helper_mem, default=0), ' MB')} for its helper (which reads Docker logs). Flat after the first minute.",
             "<b>Link cut for 60 s:</b> the agent kept 3 small files on disk and sent them 15 s after the link came back. Every line saved once.",
             "<b>Network:</b> about 0.4 KB/s for 100 lines/s, because logs are compressed."],
            [verdict("good", "Light enough to run on any server; the outage test shows no data is lost."),
             verdict("warn", "Agent plus helper is about 38 MB, a little over the 30 MB goal.")]),
]

TILES = [
    ("DuckDB", "recommended for lightweight installs"),
    (f"{fmt(duck50_mem, ' MB')} vs {fmt(chdb_flow_mem, ' MB')}", "store memory, DuckDB vs chDB, at 50 busy servers"),
    ("0", "log lines lost in any test"),
    (f"{fmt(max(agent_mem, default=0), ' MB')}", "agent memory per server (0.6 % CPU)"),
]
tiles = "".join(f'<div class="tile"><div class="v">{esc(v)}</div><div class="l">{esc(l)}</div></div>' for v, l in TILES)


# ---------- replay widget ----------
REPLAY_JSON = json.dumps(D.get("replay", []), separators=(",", ":")).replace("</", "<\\/")
REPLAY_HTML = """
<section class="card replay" aria-labelledby="rp-h">
<h2 id="rp-h">Replay a workload</h2>
<p class="muted">Pick a workload and press <b>Play</b>. Both stores get the same load and run side by side on one timeline, sped up. The grey area is when logs were being sent; after it, the store finishes saving whatever was waiting.</p>
<div class="rp-row"><span class="rp-lab">Workload</span><div class="seg" id="rp-work" role="group" aria-label="Workload"></div></div>
<div class="rp-row"><span class="rp-lab">Show</span><div class="seg" id="rp-metric" role="group" aria-label="Measure">
<button data-m="cpu">CPU</button><button data-m="mem">Memory</button><button data-m="wait">Logs waiting</button></div>
<label class="rp-chk" id="rp-mapped-l"><input type="checkbox" id="rp-mapped"> add chDB's 520 MB program file</label></div>
<p class="rp-desc" id="rp-desc"></p>
<svg id="rp-svg" viewBox="0 0 760 300" role="img" aria-label="Replay chart"></svg>
<div class="rp-bar">
<button class="rp-btn primary" id="rp-play" aria-label="Play">▶ Play</button>
<button class="rp-btn" id="rp-replay" aria-label="Replay from the start">↺ Replay</button>
<input type="range" id="rp-scrub" min="0" max="1000" value="0" aria-label="Position in the run">
<span class="rp-time" id="rp-time">0 s</span>
<div class="seg small" id="rp-speed" role="group" aria-label="Speed"><button data-s="10">10×</button><button data-s="30">30×</button><button data-s="60">60×</button></div>
</div>
<div class="rp-read" id="rp-read"></div>
<p class="muted rp-how" id="rp-how"></p>
</section>
"""
REPLAY_CSS = """
.replay{padding:22px}.rp-row{display:flex;flex-wrap:wrap;align-items:center;gap:8px 12px;margin:10px 0}
.rp-lab{font-size:13px;color:var(--ink3);width:72px}
.seg{display:inline-flex;flex-wrap:wrap;border:1px solid var(--rule);border-radius:8px;overflow:hidden;background:var(--page)}
.seg button{font:inherit;font-size:14px;border:0;background:transparent;color:var(--ink2);padding:7px 12px;cursor:pointer;border-right:1px solid var(--rule)}
.seg button:last-child{border-right:0}.seg button[aria-pressed="true"]{background:var(--s1);color:#fff}
.seg.small button{padding:5px 9px;font-size:13px}
.seg button:focus-visible,.rp-btn:focus-visible{outline:2px solid var(--s1);outline-offset:2px}
.rp-chk{font-size:13px;color:var(--ink2);display:flex;gap:6px;align-items:center}
.rp-desc{margin:4px 0 8px;color:var(--ink2);font-size:14px}
#rp-svg{background:var(--page);border:1px solid var(--rule);border-radius:10px}
.rp-bar{display:flex;flex-wrap:wrap;align-items:center;gap:10px;margin:12px 0}
.rp-btn{font:inherit;font-size:14px;border:1px solid var(--rule);background:var(--surface);color:var(--ink);padding:7px 14px;border-radius:8px;cursor:pointer}
.rp-btn.primary{background:var(--s1);border-color:var(--s1);color:#fff;min-width:96px}
#rp-scrub{flex:1;min-width:160px;accent-color:var(--s1)}.rp-time{font-variant-numeric:tabular-nums;color:var(--ink2);min-width:120px;font-size:14px}
.rp-read{display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,300px),1fr));gap:12px}
.rp-card{border:1px solid var(--rule);border-radius:10px;padding:12px 14px;background:var(--page)}
.rp-card h4{display:flex;align-items:center;gap:8px;margin:0}.rp-card .sw{width:12px;height:12px;border-radius:3px;display:inline-block}
.rp-card .note{font-size:12px;color:var(--ink3);margin:2px 0 8px}
.rp-kv{display:grid;grid-template-columns:auto 1fr;gap:3px 12px;font-size:14px;font-variant-numeric:tabular-nums}.rp-kv span:nth-child(odd){color:var(--ink3)}
.rp-res{margin-top:8px;padding:7px 10px;border-radius:7px;font-size:14px}.rp-res.good{background:var(--goodbg);color:var(--good)}.rp-res.warn{background:var(--warnbg);color:var(--warn)}
.rp-none{color:var(--ink3);font-size:14px}.rp-how{font-size:13px;margin:12px 0 0}
#rp-svg .rl{fill:none;stroke-width:2.2;stroke-linejoin:round;stroke-linecap:round}#rp-svg .ghost{fill:none;stroke-width:1;opacity:.25}
#rp-svg .ph{stroke:var(--ink);stroke-width:1;stroke-dasharray:3 3;opacity:.6}
#rp-svg text{font-size:11px;fill:var(--ink3);font-variant-numeric:tabular-nums}#rp-svg .gl{stroke:var(--rule)}#rp-svg .bd{fill:var(--band)}
#rp-svg .vl{font-size:12px;fill:var(--ink);font-weight:600}
"""
REPLAY_JS = r"""
(function(){
const W=REPLAY, svg=document.getElementById('rp-svg'); if(!W.length||!svg) return;
const STORE={chdb:{name:'chDB',col:'var(--s3)'},duck:{name:'DuckDB',col:'var(--s2)'}};
const M={cpu:{i:1,unit:'cores',label:'CPU used by the store',fmt:v=>v.toFixed(2)+' cores',how:'CPU in cores: 1.0 = one full CPU core busy. chDB = its engine plus the worker; DuckDB = the worker it lives in.'},
 mem:{i:2,unit:'MB',label:'Memory used by the store',fmt:v=>Math.round(v)+' MB',how:'RAM held by the store. chDB = engine plus worker; tick the box to add its 520 MB program file, which the OS counts but can share.'},
 wait:{i:5,unit:'files',label:'Log files waiting on the servers',fmt:v=>Math.round(v)+' files',how:'Files queued on the servers because the store was busy. Zero = keeping up. A pile that shrinks after the grey area = the store catching up. Nothing in the queue is lost.'}};
const st={w:W.findIndex(w=>w.id==='p50'),m:'cpu',t:0,play:false,speed:30,mapped:false,last:0};
if(st.w<0) st.w=0;
const $=id=>document.getElementById(id), X0=56,X1=640,Y0=16,Y1=262;
const work=$('rp-work'); W.forEach((w,i)=>{const b=document.createElement('button');b.textContent=w.label;b.dataset.i=i;work.appendChild(b)});
function press(group,attr,val){group.querySelectorAll('button').forEach(b=>b.setAttribute('aria-pressed',String(b.dataset[attr]==String(val))))}
function wk(){return W[st.w]}
function val(store,p){let v=p[M[st.m].i]; if(st.m==='mem'&&st.mapped&&store==='chdb') v+=520; return v}
function range(){const s=Object.values(wk().stores); let a=Infinity,b=-Infinity; s.forEach(r=>{a=Math.min(a,r.points[0][0]);b=Math.max(b,r.points[r.points.length-1][0])}); return [a,b]}
function ymax(){let m=0; for(const [k,r] of Object.entries(wk().stores)) r.points.forEach(p=>m=Math.max(m,val(k,p)));
 if(m<=0) return 1; const e=Math.pow(10,Math.floor(Math.log10(m))); for(const f of [1,2,2.5,5,10]) if(f*e>=m*1.08) return f*e; return m}
function at(r,t){let p=null; for(const q of r.points){if(q[0]<=t) p=q; else break} return p}
function stats(k,r,t,loadEnd){let pk=0,sum=0,n=0; for(const p of r.points){ if(p[0]>t) break; const v=val(k,p); pk=Math.max(pk,v); if(p[0]>=0&&p[0]<=loadEnd){sum+=v;n++} } return {pk,avg:n?sum/n:null}}
function fmtT(t){return (t<0?'−':'')+Math.abs(Math.round(t))+' s'}
function draw(){
 const [a,b]=range(), ym=ymax(), xs=t=>X0+(t-a)/(b-a)*(X1-X0), ys=v=>Y1-Math.min(v,ym)/ym*(Y1-Y0);
 const loadEnd=Math.max(...Object.values(wk().stores).map(r=>r.load_secs));
 let h=`<rect class="bd" x="${xs(Math.max(0,a))}" y="${Y0}" width="${xs(loadEnd)-xs(Math.max(0,a))}" height="${Y1-Y0}"/>`+
  `<text x="${xs(Math.max(0,a))+6}" y="${Y0+13}">logs being sent</text>`;
 for(let i=0;i<=4;i++){const v=ym*i/4,y=ys(v); h+=`<line class="gl" x1="${X0}" x2="${X1}" y1="${y}" y2="${y}"/><text x="${X0-6}" y="${y+4}" text-anchor="end">${+v.toFixed(2)}</text>`}
 h+=`<text x="${X0-6}" y="${Y0-4}" text-anchor="end">${M[st.m].unit}</text>`;
 const step=Math.max(60,Math.ceil((b-a)/7/60)*60); for(let t=Math.ceil(a/step)*step;t<=b;t+=step) h+=`<text x="${xs(t)}" y="${Y1+18}" text-anchor="middle">${t<0?'−':''}${Math.abs(t)} s</text>`;
 const ends=[];
 for(const [k,r] of Object.entries(wk().stores)){
  const all=r.points.map((p,i)=>(i?'L':'M')+xs(p[0]).toFixed(1)+','+ys(val(k,p)).toFixed(1)).join('');
  h+=`<path class="ghost" style="stroke:${STORE[k].col}" d="${all}"/>`;
  const seen=r.points.filter(p=>p[0]<=st.t); if(!seen.length) continue;
  h+=`<path class="rl" style="stroke:${STORE[k].col}" d="${seen.map((p,i)=>(i?'L':'M')+xs(p[0]).toFixed(1)+','+ys(val(k,p)).toFixed(1)).join('')}"/>`;
  const c=seen[seen.length-1]; h+=`<circle cx="${xs(c[0])}" cy="${ys(val(k,c))}" r="5" style="fill:${STORE[k].col};stroke:var(--page);stroke-width:2"/>`;
  ends.push([ys(val(k,c)),STORE[k].name+' '+M[st.m].fmt(val(k,c)),STORE[k].col]);
 }
 ends.sort((p,q)=>p[0]-q[0]); for(let i=1;i<ends.length;i++) if(ends[i][0]-ends[i-1][0]<16) ends[i][0]=ends[i-1][0]+16;
 ends.forEach(e=>{h+=`<text class="vl" x="${X1+10}" y="${Math.min(Y1,e[0])+4}"><tspan style="fill:${e[2]}">●</tspan> ${e[1]}</text>`});
 h+=`<line class="ph" x1="${xs(st.t)}" x2="${xs(st.t)}" y1="${Y0}" y2="${Y1}"/>`;
 svg.innerHTML=h;
 $('rp-time').textContent=fmtT(st.t)+(st.t<0?' (before load)':st.t<=loadEnd?' · load running':' · after load');
 $('rp-scrub').value=Math.round((st.t-a)/(b-a)*1000);
 let cards='';
 for(const k of ['chdb','duck']){const r=wk().stores[k];
  if(!r){cards+=`<div class="rp-card"><h4><span class="sw" style="background:${STORE[k].col}"></span>${STORE[k].name}</h4><p class="rp-none">Not measured at this workload. ${k==='chdb'?'chDB only makes sense for larger fleets.':''}</p></div>`;continue}
  const p=at(r,st.t), s=stats(k,r,st.t,r.load_secs), done=st.t>r.load_secs;
  const kept=r.acked&&r.offered&&r.acked>=0.97*r.offered;
  const res=kept?`<div class="rp-res good">✓ Kept up: saved ${Math.round(r.acked).toLocaleString()} of ${Math.round(r.offered).toLocaleString()} lines/s as they arrived${r.drain!=null?`, queue empty ${Math.round(r.drain)} s after the load`:''}.</div>`
   :`<div class="rp-res warn">! Fell behind: saved ${Math.round(r.acked).toLocaleString()} of ${Math.round(r.offered).toLocaleString()} lines/s (${Math.round(r.acked/r.offered*100)} %) as they arrived; the rest waited on the servers${r.drain!=null?` and was saved ${Math.round(r.drain)} s after the load`:''}. No lines lost.</div>`;
  cards+=`<div class="rp-card"><h4><span class="sw" style="background:${STORE[k].col}"></span>${STORE[k].name}</h4><div class="note">${r.note||'same load, no limits'}</div>
  <div class="rp-kv"><span>Now</span><span>${p?M[st.m].fmt(val(k,p)):'–'}</span><span>Peak so far</span><span>${M[st.m].fmt(s.pk)}</span>
  <span>Average while sending</span><span>${s.avg==null?'–':M[st.m].fmt(s.avg)}</span>
  <span>forge-server now</span><span>${p?p[3].toFixed(2)+' cores, '+Math.round(p[4])+' MB':'–'}</span>
  <span>Search (slowest 5 %)</span><span>${r.search_p95!=null?(r.search_p95/1000).toFixed(1)+' s':'–'}</span></div>${done?res:''}</div>`}
 $('rp-read').innerHTML=cards; $('rp-desc').textContent=wk().desc+' Showing: '+M[st.m].label+'.'; $('rp-how').textContent='How to read: '+M[st.m].how;
 $('rp-mapped-l').style.display=st.m==='mem'?'flex':'none';
 $('rp-play').textContent=st.play?'❚❚ Pause':(st.t>=range()[1]?'▶ Play again':'▶ Play'); $('rp-play').setAttribute('aria-label',st.play?'Pause':'Play');
 press(work,'i',st.w); press($('rp-metric'),'m',st.m); press($('rp-speed'),'s',st.speed);
}
function tick(ts){ if(!st.play) return; const dt=st.last?(ts-st.last)/1000:0; st.last=ts; st.t+=dt*st.speed;
 const [,b]=range(); if(st.t>=b){st.t=b;st.play=false} draw(); if(st.play) requestAnimationFrame(tick)}
function start(){ if(st.t>=range()[1]) st.t=range()[0]; st.play=true; st.last=0; requestAnimationFrame(tick); draw()}
function reset(){st.play=false; st.t=range()[0]; draw()}
$('rp-play').onclick=()=>{ if(st.play){st.play=false;draw()} else start()};
$('rp-replay').onclick=()=>{reset(); start()};
$('rp-scrub').oninput=e=>{const [a,b]=range(); st.play=false; st.t=a+(b-a)*e.target.value/1000; draw()};
work.onclick=e=>{const b=e.target.closest('button'); if(!b) return; st.w=+b.dataset.i; reset()};
$('rp-metric').onclick=e=>{const b=e.target.closest('button'); if(!b) return; st.m=b.dataset.m; draw()};
$('rp-speed').onclick=e=>{const b=e.target.closest('button'); if(!b) return; st.speed=+b.dataset.s; draw()};
$('rp-mapped').onchange=e=>{st.mapped=e.target.checked; draw()};
st.t=range()[1]; draw();
})();
"""

CSS = """
:root{--surface:#fcfcfb;--page:#f4f3f0;--ink:#0b0b0b;--ink2:#52514e;--ink3:#7a7974;--rule:#e3e2dd;--band:#ecebe6;--tint:#eef4fc;
--s1:#2a78d6;--s2:#eb6834;--s3:#1baf7a;--s4:#eda100;--s5:#e87ba4;--s6:#6250d6;--crit:#d03b3b;--good:#0a7f0a;--goodbg:#e8f5e8;--warn:#9a6700;--warnbg:#fdf3dc;--badbg:#fbe9e9}
@media (prefers-color-scheme:dark){:root:not([data-theme="light"]){--surface:#1a1a19;--page:#121211;--ink:#f0efec;--ink2:#c3c2b7;--ink3:#8f8e86;--rule:#2e2e2b;--band:#232321;--tint:#1b2533;
--s1:#3987e5;--s2:#d95926;--s3:#199e70;--s4:#c98500;--s5:#d55181;--s6:#9085e9;--crit:#e66767;--good:#4cc24c;--goodbg:#17261a;--warn:#e0a92e;--warnbg:#2b2414;--badbg:#2d1a1a}}
:root[data-theme="dark"]{--surface:#1a1a19;--page:#121211;--ink:#f0efec;--ink2:#c3c2b7;--ink3:#8f8e86;--rule:#2e2e2b;--band:#232321;--tint:#1b2533;
--s1:#3987e5;--s2:#d95926;--s3:#199e70;--s4:#c98500;--s5:#d55181;--s6:#9085e9;--crit:#e66767;--good:#4cc24c;--goodbg:#17261a;--warn:#e0a92e;--warnbg:#2b2414;--badbg:#2d1a1a}
*{box-sizing:border-box}body{margin:0;background:var(--page);color:var(--ink);font:16px/1.6 system-ui,-apple-system,"Segoe UI",sans-serif}
main{max-width:1040px;margin:0 auto;padding:40px 16px 80px}
h1{font-size:30px;line-height:1.2;margin:0 0 8px}h2{font-size:22px;line-height:1.3;margin:4px 0 8px}h3{font-size:15px;margin:0}h4{margin:0 0 6px;font-size:15px}
p{max-width:760px}a{color:var(--s1)}.muted{color:var(--ink2)}
.answer{background:var(--tint);border:1px solid var(--rule);border-left:4px solid var(--s1);border-radius:10px;padding:18px 20px;margin:20px 0}
.answer p{margin:6px 0}
.tiles{display:grid;grid-template-columns:repeat(auto-fit,minmax(200px,1fr));gap:12px;margin:20px 0 8px}
.tile{background:var(--surface);border:1px solid var(--rule);border-radius:10px;padding:14px 16px}
.tile .v{font-size:22px;font-weight:700;font-variant-numeric:tabular-nums}.tile .l{color:var(--ink2);font-size:14px}
.card{background:var(--surface);border:1px solid var(--rule);border-radius:12px;padding:18px 20px;margin:12px 0}
.part{margin-top:48px;font-size:13px;letter-spacing:.06em;text-transform:uppercase;color:var(--ink3);font-weight:600}
.flow{width:100%;height:auto}.flow .box rect{fill:var(--surface);stroke:var(--rule);stroke-width:1.5}.flow .box.hi rect{stroke:var(--s1);stroke-width:2}
.flow text{fill:var(--ink);font-size:15px;font-weight:600;text-anchor:middle}.flow text.sm{fill:var(--ink2);font-size:12px;font-weight:400}
.flow .arw{stroke:var(--ink3);stroke-width:2;fill:none}.flow .dash{stroke-dasharray:5 4}.flow .arh{fill:var(--ink3)}
.steps{margin:12px 0 0;padding-left:20px}.steps li{margin:4px 0}
dl.gl{display:grid;grid-template-columns:minmax(120px,190px) 1fr;gap:8px 18px;margin:0}dl.gl dt{font-weight:600}dl.gl dd{margin:0;color:var(--ink2)}
@media (max-width:560px){dl.gl{grid-template-columns:1fr}dl.gl dd{margin-bottom:8px}}
.tw{overflow-x:auto}table{border-collapse:collapse;width:100%;font-size:14px;font-variant-numeric:tabular-nums}
th,td{text-align:left;padding:9px 10px;border-bottom:1px solid var(--rule);vertical-align:top}th{color:var(--ink2);font-weight:600;font-size:13px}
.pill{display:inline-block;padding:2px 10px;border-radius:999px;font-size:13px}.pill.good{background:var(--goodbg);color:var(--good)}.pill.warn{background:var(--warnbg);color:var(--warn)}
.test{background:var(--surface);border:1px solid var(--rule);border-radius:14px;padding:22px;margin:20px 0}
.qn{font-size:12px;font-weight:700;letter-spacing:.06em;text-transform:uppercase;color:var(--s1)}
.setup{color:var(--ink2);margin:4px 0 14px}
.grid2{display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,440px),1fr));gap:14px;margin-bottom:14px}
.chart{margin:0;background:var(--page);border:1px solid var(--rule);border-radius:10px;padding:14px}
.chart figcaption p{margin:2px 0 6px;color:var(--ink2);font-size:13px}
svg{width:100%;height:auto;display:block}
.grid{stroke:var(--rule);stroke-width:1}.axis{stroke:var(--ink3);stroke-width:1}
.tick,.unit{fill:var(--ink3);font-size:11px;font-variant-numeric:tabular-nums}
.band{fill:var(--band)}.bandlabel{fill:var(--ink3);font-size:11px}
.line{fill:none;stroke-width:2;stroke-linejoin:round;stroke-linecap:round}
.hit{fill:transparent;stroke:none;cursor:crosshair}.hit:hover{stroke:var(--surface);stroke-width:2}
.s1{stroke:var(--s1)}.s2{stroke:var(--s2)}.s3{stroke:var(--s3)}.s4{stroke:var(--s4)}.s5{stroke:var(--s5)}.s6{stroke:var(--s6)}
.hit.s1:hover,.bar.s1,.key.s1{fill:var(--s1)}.hit.s2:hover,.bar.s2,.key.s2{fill:var(--s2)}.hit.s3:hover,.bar.s3,.key.s3{fill:var(--s3)}
.hit.s4:hover,.bar.s4,.key.s4{fill:var(--s4)}.hit.s5:hover,.bar.s5,.key.s5{fill:var(--s5)}.hit.s6:hover,.bar.s6,.key.s6{fill:var(--s6)}
.bar{stroke:none}.bar:hover{opacity:.8}.dlabel{fill:var(--ink2);font-size:12px}.key{stroke:none}
.threshold{stroke:var(--crit);stroke-width:1.5;stroke-dasharray:4 4}.thlabel{fill:var(--crit);font-size:11px}.over{fill:var(--ink2);font-size:10px}
details{margin-top:8px;font-size:13px}summary{cursor:pointer;color:var(--ink2)}
.find{border-top:1px solid var(--rule);padding-top:12px}.find ul{margin:0;padding-left:20px}.find li{margin:6px 0}
.verdict{margin-top:10px;padding:10px 14px;border-radius:8px;font-size:15px}.verdict .vi{display:inline-block;width:20px;font-weight:700}
.verdict.good{background:var(--goodbg)}.verdict.good .vi,.verdict.good b{color:var(--good)}
.verdict.warn{background:var(--warnbg)}.verdict.warn .vi,.verdict.warn b{color:var(--warn)}
.verdict.bad{background:var(--badbg)}.verdict.bad .vi,.verdict.bad b{color:var(--crit)}
ul.runs{list-style:none;padding:0;margin:0}ul.runs li{padding:6px 0;border-bottom:1px solid var(--rule)}ul.runs .d{color:var(--ink3);font-variant-numeric:tabular-nums;margin-right:8px}
#tip{position:fixed;pointer-events:none;background:var(--ink);color:var(--surface);padding:6px 9px;border-radius:6px;font-size:12px;opacity:0;transition:opacity .08s;z-index:9;max-width:280px}
.foot{color:var(--ink3);font-size:13px;margin-top:40px}
"""

page = f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Forge Log Benchmarks</title><style>{CSS}{REPLAY_CSS}</style></head><body><main>
<h1>How much does Forge's log system cost, and how far does it go?</h1>
<p class="muted">Benchmarks of the new log pipeline, written so anyone can follow them. All tests ran on one machine: 16 CPU cores, 62 GB RAM, Linux.</p>

<div class="answer"><h4>The short answer</h4>
<p>For a <b>lightweight install (2–5 servers)</b>, the log system is far from its limits. Our tests used 10 to 100 times more logging than such an install produces.</p>
<p>Use <b>DuckDB</b> as the log store there: it needs about a third of chDB's memory and no extra program. Keep <b>chDB</b> (or an external ClickHouse) for big fleets, where it scales further.</p>
<p>In every test, <b>no log line was lost</b>. When the store is busy, logs wait on the servers' own disks and arrive a little later.</p></div>
<div class="tiles">{tiles}</div>
{REPLAY_HTML}

<div class="part">Part 1 · How it works</div>
<div class="card"><h2>Where a log line goes</h2>{FLOW}
<ol class="steps"><li>An app writes a log line. <b>forge-agent</b> on that server collects it.</li>
<li>The agent packs lines into small compressed files on its own disk (the <b>spool</b>) and uploads each file to the <b>analytics worker</b>.</li>
<li>The worker saves the lines into the <b>log store</b>: chDB or DuckDB. Only after that does the agent delete its file, so nothing is lost if anything restarts.</li>
<li><b>forge-server</b> runs the web console. When you search logs it asks the worker; it never holds the logs itself.</li></ol></div>
<div class="card"><h2>Words used in this report</h2><dl class="gl">{gloss}</dl></div>

<div class="part">Part 2 · What to use</div>
<div class="card"><h2>Which store for which install</h2><div class="tw"><table><thead><tr><th>Install</th><th>Typical size</th><th>Use</th><th>What to expect</th></tr></thead><tbody>{sizing}</tbody></table></div></div>
<div class="card"><h2>chDB vs DuckDB at a glance</h2><div class="tw"><table><thead><tr><th></th><th>chDB</th><th>DuckDB</th><th>Better</th></tr></thead><tbody>{cmp_rows}</tbody></table></div></div>

<div class="part">Part 3 · The tests</div>
{''.join(tests)}

<div class="part">Part 4 · Still open</div>
<div class="card"><ul>
<li><b>DuckDB's CPU at light load: explained.</b> The {cores(dl.get('worker_cpu'))} core with 5 servers was the test's 4 non-stop searchers. Saving the logs alone costs about 0.005 core, and an idle DuckDB uses 0 % CPU and 44 MB.</li>
<li><b>Searches at 100 busy servers</b> stay slow on DuckDB (log chart about {fmt((d100t.get('hist_p95') or 0) / 1000, ' s')}), because it has no text index. Use chDB or ClickHouse when many people search large fleets.</li>
<li><b>Longer runs:</b> every test lasted minutes. A one-hour run will confirm that CPU and memory stay flat.</li>
<li><b>Bigger writes</b> (grouping several uploaded files into one save) should cut CPU on both stores.</li>
<li><b>forge-server memory</b> is 66–83 MB at 50 servers and grows about 60 KB per extra server; the goal is under 100 MB for typical fleets.</li>
</ul></div>

<div class="part">Detailed reports</div>
<div class="card"><ul class="runs">{run_list}</ul>
<p class="muted" style="margin-bottom:0">Every chart has a <b>Data table</b> link under it, and every point shows its value on hover. Full method, settings and per-phase tables are in each report and <a href="README.md">README.md</a>.</p></div>
<p class="foot">Built by scripts/dev/benchmarks/report/build_report.py from report/data.json.</p>
</main><div id="tip" role="tooltip"></div>
<script>const REPLAY={REPLAY_JSON};{REPLAY_JS}</script>
<script>
const tip=document.getElementById('tip');
document.addEventListener('pointermove',e=>{{const t=e.target.closest('[data-tip]');
 if(!t){{tip.style.opacity=0;return}} tip.textContent=t.dataset.tip;tip.style.opacity=1;
 const x=Math.min(e.clientX+14,innerWidth-tip.offsetWidth-8);tip.style.left=x+'px';tip.style.top=(e.clientY+14)+'px'}});
</script></body></html>
"""
OUT.write_text(page)
print(f"wrote {OUT} ({len(page) // 1024} KB)")
