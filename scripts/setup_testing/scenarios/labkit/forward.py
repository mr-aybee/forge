"""Plain TCP port forwarding, so WSL can reach the lab through Windows.

WSL in mirrored mode shares Windows' localhost but has no route to the
Hyper-V lab network (10.50.0.0/24). Run this on Windows (no admin needed):

    python labkit/forward.py 18080=10.50.0.10:8080 18081=10.50.0.10:80

then in WSL use `--server http://127.0.0.1:18080 --edge 127.0.0.1:18081`.
The listeners bind 127.0.0.1 only. Ctrl+C stops it.
"""

from __future__ import annotations

import socket
import sys
import threading


def pipe(src: socket.socket, dst: socket.socket) -> None:
    try:
        while data := src.recv(65536):
            dst.sendall(data)
    except OSError:
        pass
    finally:
        for s in (src, dst):
            try:
                s.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            s.close()


def serve(local_port: int, host: str, port: int) -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", local_port))
    srv.listen(128)
    print(f"127.0.0.1:{local_port} → {host}:{port}", flush=True)
    while True:
        client, _ = srv.accept()
        try:
            upstream = socket.create_connection((host, port), timeout=10)
            upstream.settimeout(None)
        except OSError:
            client.close()
            continue
        threading.Thread(target=pipe, args=(client, upstream), daemon=True).start()
        threading.Thread(target=pipe, args=(upstream, client), daemon=True).start()


def main() -> None:
    rules = sys.argv[1:] or ["18080=10.50.0.10:8080", "18081=10.50.0.10:80"]
    for r in rules:
        local, remote = r.split("=", 1)
        host, port = remote.rsplit(":", 1)
        threading.Thread(target=serve, args=(int(local), host, int(port)), daemon=True).start()
    try:
        threading.Event().wait()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
