"""Samples forge-server, the analytics worker, its embedded engine (chDB)
and fleet-sim every 5 s, plus the spool backlog on the simulated nodes and
the rows stored in the engine.

Usage: python3 -I sample.py <bench_dir> <out.jsonl>
Runs until killed. Each line is one JSON object.
"""
import base64, json, os, sys, time, urllib.request

B, OUT = sys.argv[1:3]
DATA = os.path.join(B, "data")
TICK = os.sysconf("SC_CLK_TCK")
NCPU = os.cpu_count()
EVERY = 5.0
ENGINE = "http://127.0.0.1:18124/"


def pids():
    found = {}
    for p in os.listdir("/proc"):
        if not p.isdigit():
            continue
        try:
            cmd = open(f"/proc/{p}/cmdline", "rb").read().split(b"\0")
        except OSError:
            continue
        exe = os.path.basename(cmd[0].decode(errors="replace")) if cmd and cmd[0] else ""
        args = b" ".join(cmd).decode(errors="replace")
        if exe == "forge-server" and " serve" in args and B in args:
            found["server"] = int(p)
        elif exe == "forge-analytics-worker":
            found["worker"] = int(p)
        elif exe == "clickhouse" and DATA in args:
            found["engine"] = int(p)
        elif exe == "fleet-sim":
            found["sim"] = int(p)
    return found


def proc(pid):
    try:
        stat = open(f"/proc/{pid}/stat").read()
        f = stat[stat.rfind(")") + 2:].split()
        cpu = (int(f[11]) + int(f[12])) / TICK
        st = {}
        for line in open(f"/proc/{pid}/status"):
            k, _, v = line.partition(":")
            if k in ("VmRSS", "RssAnon", "RssFile", "Threads"):
                st[k] = int(v.split()[0])
            elif k == "Cpus_allowed_list":
                st[k] = v.strip()
        try:
            st["fds"] = len(os.listdir(f"/proc/{pid}/fd"))
        except OSError:
            pass
        return cpu, st
    except (OSError, IndexError, ValueError):
        return None


def spools():
    files = size = 0
    nodes = os.path.join(B, "nodes")
    try:
        dirs = os.listdir(nodes)
    except OSError:
        return 0, 0
    for d in dirs:
        sp = os.path.join(nodes, d, "spool")
        try:
            with os.scandir(sp) as it:
                for e in it:
                    if e.name.endswith(".ready"):
                        files += 1
                        try:
                            size += e.stat().st_size
                        except OSError:
                            pass
        except OSError:
            pass
    return files, size


def dir_mb(path):
    """Size of the files under `path`, MB."""
    total = 0
    for root, _, files in os.walk(path):
        for f in files:
            try:
                total += os.stat(os.path.join(root, f)).st_size
            except OSError:
                pass
    return round(total / 2**20, 1)


def write_bytes(pid):
    """Bytes the process caused to be written to storage (/proc/<pid>/io)."""
    try:
        for line in open(f"/proc/{pid}/io"):
            if line.startswith("write_bytes:"):
                return int(line.split()[1])
    except (OSError, ValueError):
        pass
    return None


def lo_bytes():
    """Bytes received on the loopback interface (every local connection)."""
    try:
        for line in open("/proc/net/dev"):
            name, _, rest = line.partition(":")
            if name.strip() == "lo":
                return int(rest.split()[0])
    except (OSError, ValueError, IndexError):
        pass
    return None


def rows():
    try:
        pw = open(os.path.join(DATA, "analytics", "secret")).read().strip()
        q = "SELECT count() FROM forge_op_analytics.app_logs WHERE source = 'container' FORMAT TSV"
        req = urllib.request.Request(ENGINE, q.encode(), {
            "Authorization": "Basic " + base64.b64encode(f"forge:{pw}".encode()).decode()})
        return int(urllib.request.urlopen(req, timeout=10).read().strip())
    except Exception:  # engine busy or down: recorded as None
        return None


def main():
    prev, out = {}, open(OUT, "a", buffering=1)
    while True:
        t0 = time.time()
        row = {"t": round(t0, 1)}
        for name, pid in pids().items():
            p = proc(pid)
            if not p:
                continue
            cpu, st = p
            if name in prev and prev[name][0] == pid:
                d = (cpu - prev[name][1]) / (t0 - prev[name][2])
                row[f"{name}_cpu_core_pct"] = round(d * 100, 1)
            prev[name] = (pid, cpu, t0)
            row[f"{name}_rss_mb"] = round(st.get("VmRSS", 0) / 1024, 1)
            row[f"{name}_anon_mb"] = round(st.get("RssAnon", 0) / 1024, 1)
            row[f"{name}_file_mb"] = round(st.get("RssFile", 0) / 1024, 1)
            row[f"{name}_fds"] = st.get("fds")
            row[f"{name}_threads"] = st.get("Threads")
            row[f"{name}_pid"] = pid
            if name == "engine":
                row["engine_cpus"] = st.get("Cpus_allowed_list")
            if name in ("worker", "engine"):
                row[f"{name}_write_bytes"] = write_bytes(pid)
        row["spool_files"], sb = spools()
        row["spool_mb"] = round(sb / 2**20, 2)
        row["rows"] = rows()
        row["analytics_mb"] = dir_mb(os.path.join(DATA, "analytics"))
        row["lo_rx_bytes"] = lo_bytes()
        out.write(json.dumps(row) + "\n")
        time.sleep(max(0.0, EVERY - (time.time() - t0)))


main()
