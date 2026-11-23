#!/usr/bin/env python3
"""Samples memory and CPU of forge-server, forge-agent and forge-agent-helper
(plus whole-VM memory/CPU) on the lab VMs every N seconds, until stopped.

One SSH call per VM per sample reads /proc directly, so the measurement does
not go through the Forge agent (which would skew the agent's own numbers).
The SSH login comes from scripts/setup_testing/.lab/vm-lab.env (never printed).

Output (append-only, safe to stop with Ctrl-C and restart):
  <out-dir>/samples.csv   ts_utc,vm,kind,name,pid,v1,v2
      kind P = process:  v1 = RSS KiB,          v2 = utime+stime ticks
      kind M = memory:   v1 = MemTotal KiB,     v2 = MemAvailable KiB
      kind C = cpu:      v1 = total jiffies,    v2 = idle+iowait jiffies
      kind T = info:     v1 = CLK_TCK,          v2 = nproc, name = load1
  <out-dir>/specs.txt     VM specs captured at start (Hyper-V on Windows + inside the VM)

Examples:
  python3 scripts/setup_testing/perf/monitor.py
  python3 scripts/setup_testing/perf/monitor.py --interval 10 --out-dir ~/forge-perf
"""
from __future__ import annotations

import argparse
import datetime as dt
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import time
from pathlib import Path

LAB = Path(__file__).resolve().parents[1] / ".lab"
WINDOWS = os.name == "nt"
DEFAULT_OUT = Path(tempfile.gettempdir()) / "forge-perf"

# Runs on the VM as the unprivileged lab user: /proc/<pid>/{cmdline,status,stat} are world-readable.
PROBE = r"""t=$(getconf CLK_TCK)
for d in /proc/[0-9]*; do
  a=$(tr '\0' ' ' < $d/cmdline 2>/dev/null | cut -d' ' -f1); n=${a##*/}
  case "$n" in forge-server|forge-agent|forge-agent-helper)
    p=${d#/proc/}
    r=$(awk '/^VmRSS/{print $2}' $d/status 2>/dev/null)
    c=$(awk '{print $14+$15}' $d/stat 2>/dev/null)
    [ -n "$r" ] && echo "P,$n,$p,$r,$c";;
  esac
done
awk '/^MemTotal/{t=$2}/^MemAvailable/{a=$2}END{print "M,mem,,"t","a}' /proc/meminfo
awk '/^cpu /{s=0;for(k=2;k<=NF;k++)s+=$k;print "C,cpu,,"s","$5+$6}' /proc/stat
echo "T,$(cut -d' ' -f1 /proc/loadavg),,$t,$(nproc)"
"""

SPECS = (r'''echo "os=$(. /etc/os-release; echo $PRETTY_NAME) kernel=$(uname -r) cpu=$(nproc)x '''
         r'''$(awk -F: "/model name/{print \$2; exit}" /proc/cpuinfo) mem_total_kib=$(awk "/MemTotal/{print \$2}" /proc/meminfo) '''
         r'''forge_server=$(forge-server --version 2>/dev/null | head -1) forge_agent=$(forge-agent --version 2>/dev/null | head -1)"''')


def read_env(path: Path) -> dict[str, str]:
    out: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if line and not line.startswith("#") and "=" in line:
            k, v = line.split("=", 1)
            out[k.strip()] = v.strip()
    return out


def ssh_setup(password: str) -> tuple[str, dict[str, str]]:
    """Non-interactive password login through SSH_ASKPASS (same as vm_lab.py)."""
    LAB.mkdir(exist_ok=True)
    if WINDOWS:
        askpass = LAB / "askpass.cmd"
        if not askpass.exists():
            askpass.write_text("@echo off\r\necho %FORGE_LAB_PASSWORD%\r\n", encoding="ascii")
        win = Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32" / "OpenSSH" / "ssh.exe"
        ssh = str(win) if win.is_file() else (shutil.which("ssh") or "ssh")
    else:
        askpass = LAB / "askpass.sh"
        if not askpass.exists():
            askpass.write_text('#!/bin/sh\nprintf \'%s\\n\' "$FORGE_LAB_PASSWORD"\n', encoding="ascii")
        askpass.chmod(stat.S_IRWXU)
        ssh = shutil.which("ssh") or "ssh"
    env = dict(os.environ, FORGE_LAB_PASSWORD=password, SSH_ASKPASS=str(askpass), SSH_ASKPASS_REQUIRE="force")
    env.setdefault("DISPLAY", "forge-lab")
    return ssh, env


def hyperv_specs(name: str) -> str:
    if not WINDOWS:
        return "not readable here (Hyper-V is Windows only)"
    ps = (f"$vm = Get-VM -Name '{name}' -ErrorAction Stop; "
          f"$disk = (Get-VMHardDiskDrive -VMName '{name}' | ForEach-Object {{ (Get-VHD $_.Path).Size }} | Measure-Object -Sum).Sum / 1GB; "
          "\"gen $($vm.Generation), vCPU $($vm.ProcessorCount), memory startup $($vm.MemoryStartup/1MB) MB, "
          "dynamic $($vm.DynamicMemoryEnabled) (min $($vm.MemoryMinimum/1MB) / max $($vm.MemoryMaximum/1MB) MB), "
          "disk $([math]::Round($disk,1)) GB\"")
    p = subprocess.run([shutil.which("pwsh") or "powershell", "-NoProfile", "-Command", ps], capture_output=True, text=True)
    return p.stdout.strip() if p.returncode == 0 else f"not readable here ({p.stderr.strip()})"


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--interval", type=int, default=10, help="seconds between samples")
    ap.add_argument("--out-dir", type=Path, default=DEFAULT_OUT)
    ap.add_argument("--vms", nargs="+", default=["lab-a=10.50.0.10", "lab-b=10.50.0.11", "lab-c=10.50.0.12"])
    a = ap.parse_args()

    env_file = LAB / "vm-lab.env"
    if not env_file.exists():
        sys.exit(f"{env_file} is missing: run `python3 scripts/setup_testing/vm_lab.py init`")
    cfg = read_env(env_file)
    if not cfg.get("FORGE_LAB_PASSWORD"):
        sys.exit(f"FORGE_LAB_PASSWORD is not set in {env_file}")
    user = cfg.get("FORGE_LAB_USER") or "forge"
    ssh, env = ssh_setup(cfg["FORGE_LAB_PASSWORD"])
    ssh_args = ["-o", "StrictHostKeyChecking=accept-new", "-o", f"UserKnownHostsFile={LAB / 'known_hosts'}",
                "-o", "ConnectTimeout=5", "-o", "LogLevel=ERROR", "-o", "PreferredAuthentications=publickey,password"]
    vms = [tuple(v.split("=", 1)) for v in a.vms]

    def remote(ip: str, cmd: str, timeout: int = 30) -> subprocess.CompletedProcess:
        try:
            return subprocess.run([ssh, *ssh_args, f"{user}@{ip}", cmd], env=env, capture_output=True, text=True,
                                  stdin=subprocess.DEVNULL, timeout=timeout)
        except subprocess.TimeoutExpired:
            return subprocess.CompletedProcess([], 124, "", "timeout")

    out_dir: Path = a.out_dir.expanduser().resolve()
    out_dir.mkdir(parents=True, exist_ok=True)
    csv = out_dir / "samples.csv"
    if not csv.exists():
        csv.write_text("ts_utc,vm,kind,name,pid,v1,v2\n", encoding="utf-8")

    # Specs, once per start (appended, so restarts are visible).
    with (out_dir / "specs.txt").open("a", encoding="utf-8") as specs:
        specs.write(f"=== {dt.datetime.now().astimezone().isoformat()} monitor start, interval {a.interval}s ===\n")
        for name, ip in vms:
            specs.write(f"[{name}] Hyper-V: {hyperv_specs(name)}\n")
            r = remote(ip, SPECS)
            specs.write(f"[{name}] inside: {(r.stdout + r.stderr).strip()}\n")

    print(f"Sampling {len(vms)} VMs every {a.interval}s -> {csv}  (Ctrl-C to stop)")
    try:
        while True:
            start = time.monotonic()
            ts = dt.datetime.now(dt.timezone.utc).isoformat()
            lines: list[str] = []
            for name, ip in vms:
                r = remote(ip, PROBE)
                if r.returncode != 0 or not r.stdout.strip():
                    lines.append(f"{ts},{name},E,unreachable,,,")
                    continue
                for l in r.stdout.splitlines():
                    if len(l) > 1 and l[0] in "PMCT" and l[1] == ",":
                        f = l.split(",")
                        lines.append(f"{ts},{name},{f[0]},{f[1]},{f[2]},{f[3]},{f[4]}")
            with csv.open("a", encoding="utf-8") as out:
                out.write("\n".join(lines) + "\n")
            wait = a.interval - (time.monotonic() - start)
            if wait > 0:
                time.sleep(wait)
    except KeyboardInterrupt:
        print("stopped")


if __name__ == "__main__":
    main()
