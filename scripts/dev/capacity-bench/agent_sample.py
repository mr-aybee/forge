"""Samples one real forge-agent (and its root helper) every 5 s: CPU, RSS,
fds, spool files and bytes, bytes sent per peer port (from `ss`, the agent
keeps one HTTP/2 connection per endpoint) and /proc/<pid>/io. Every 0.25 s
it hard-links each new sealed spool file into <archive>, so the files'
headers can be counted after the agent deleted them on ack.

Usage: python3 -I agent_sample.py <agent_state_dir> <out.jsonl> <archive_dir>
Runs until killed.
"""
import json, os, re, subprocess, sys, time

STATE, OUT, ARCH = sys.argv[1:4]
SPOOL = os.path.join(STATE, "spool")
TICK = os.sysconf("SC_CLK_TCK")
EVERY, FAST = 5.0, 0.25
os.makedirs(ARCH, exist_ok=True)


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
        if STATE not in args:
            continue
        if exe == "forge-agent" and " run" in args:
            found["agent"] = int(p)
        elif exe == "forge-agent-helper" and " serve" in args:
            found["helper"] = int(p)
    return found


def proc(pid):
    try:
        stat = open(f"/proc/{pid}/stat").read()
        f = stat[stat.rfind(")") + 2:].split()
        cpu = (int(f[11]) + int(f[12])) / TICK
        st = {}
        for line in open(f"/proc/{pid}/status"):
            k, _, v = line.partition(":")
            if k in ("VmRSS", "RssAnon", "RssFile", "VmHWM", "Threads"):
                st[k] = int(v.split()[0])
        try:
            st["fds"] = len(os.listdir(f"/proc/{pid}/fd"))
        except OSError:  # the helper runs as root
            pass
        try:
            for line in open(f"/proc/{pid}/io"):
                k, _, v = line.partition(":")
                st["io_" + k] = int(v)
        except OSError:
            pass
        return cpu, st
    except (OSError, IndexError, ValueError):
        return None


def spool():
    files = ready = size = 0
    try:
        with os.scandir(SPOOL) as it:
            for e in it:
                try:
                    size += e.stat().st_size
                except OSError:
                    continue
                files += 1
                if e.name.endswith(".ready"):
                    ready += 1
    except OSError:
        pass
    return files, ready, size


def archive():
    try:
        names = os.listdir(SPOOL)
    except OSError:
        return
    for n in names:
        if n.endswith(".ready") and not os.path.exists(os.path.join(ARCH, n)):
            try:
                os.link(os.path.join(SPOOL, n), os.path.join(ARCH, n))
            except OSError:
                pass  # acked and deleted between list and link: counted from seen.log as missed


SOCK = re.compile(r"\S+\s+\S+\s+(\S+):(\d+)\s+(\S+):(\d+).*?pid=(\d+)")


def sockets(pid, seen):
    """Updates seen[(local port, peer port)] = bytes_sent / bytes_received."""
    try:
        out = subprocess.run(["ss", "-tinpH", "state", "established"], capture_output=True, text=True, timeout=5).stdout
    except (OSError, subprocess.SubprocessError):
        return
    lines = out.splitlines()
    for i, line in enumerate(lines):
        m = SOCK.search(line)
        if not m or int(m.group(5)) != pid:
            continue
        info = lines[i + 1] if i + 1 < len(lines) else ""
        sent = re.search(r"bytes_sent:(\d+)", info)
        recv = re.search(r"bytes_received:(\d+)", info)
        seen[(int(m.group(2)), int(m.group(4)))] = (int(sent.group(1)) if sent else 0, int(recv.group(1)) if recv else 0)


def main():
    prev, socks, out = {}, {}, open(OUT, "a", buffering=1)
    nxt = time.time()
    while True:
        archive()
        now = time.time()
        if now < nxt:
            time.sleep(FAST)
            continue
        nxt = now + EVERY
        row = {"t": round(now, 1)}
        for name, pid in pids().items():
            p = proc(pid)
            if not p:
                continue
            cpu, st = p
            if name in prev and prev[name][0] == pid:
                row[f"{name}_cpu_core_pct"] = round((cpu - prev[name][1]) / (now - prev[name][2]) * 100, 1)
            prev[name] = (pid, cpu, now)
            row[f"{name}_pid"] = pid
            for k, v in st.items():
                if k in ("VmRSS", "RssAnon", "RssFile", "VmHWM"):
                    row[f"{name}_{k}_mb"] = round(v / 1024, 1)
                else:
                    row[f"{name}_{k}"] = v
            if name == "agent":
                sockets(pid, socks)
        for peer in (56131, 56132):
            row[f"sent_to_{peer}"] = sum(v[0] for k, v in socks.items() if k[1] == peer)
            row[f"recv_from_{peer}"] = sum(v[1] for k, v in socks.items() if k[1] == peer)
            row[f"conns_{peer}"] = len([k for k in socks if k[1] == peer])
        row["spool_files"], row["spool_ready"], sb = spool()
        row["spool_bytes"] = sb
        out.write(json.dumps(row) + "\n")


main()
