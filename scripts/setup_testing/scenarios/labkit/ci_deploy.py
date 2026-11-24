"""What a CI job does after it published a release: deploy it with Forge.

    python ci_deploy.py --template forge.deploy.yaml --var TAG=v2 --var SHA256=<hex> \
        --token-file ci.token [--server http://10.50.0.10:3880] [--environment prod]

Fills `${NAME}` placeholders in the template from `--var NAME=value`, then
runs `forge deploy apply -f <file> --wait` with the token in FORGE_TOKEN (as
a GitHub Actions secret would be), and exits with forge's exit code: 0 when
the release is live, 10 when the rollout failed or was rolled back, 4 when
the token may not deploy this service.
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def render(text: str, values: dict[str, str]) -> str:
    def sub(m: re.Match) -> str:
        key = m.group(1)
        if key not in values:
            sys.exit(f"the template needs --var {key}=…")
        return values[key]

    return re.sub(r"\$\{([A-Z0-9_]+)\}", sub, text)


def deploy(template: str, values: dict[str, str], token: str, *, forge: str = "forge", server: str | None = None,
           environment: str | None = None, timeout_minutes: int = 30) -> int:
    with tempfile.TemporaryDirectory() as tmp:
        f = Path(tmp) / "forge.deploy.yaml"
        f.write_text(render(template, values))
        cmd = [shutil.which(forge) or forge, "--non-interactive", "deploy", "apply", "-f", str(f), "--wait",
               "--timeout", str(timeout_minutes)]
        if environment:
            cmd += ["--environment", environment]
        env = dict(os.environ, FORGE_TOKEN=token)
        if server:
            env["FORGE_SERVER"] = server
            if server.startswith("http://"):
                env.setdefault("FORGE_ALLOW_HTTP", "1")
        return subprocess.run(cmd, env=env).returncode


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--template", required=True, type=Path)
    p.add_argument("--var", action="append", default=[], help="NAME=value; repeatable")
    p.add_argument("--token-file", type=Path, help="file with the deploy token (default: FORGE_TOKEN)")
    p.add_argument("--server", default=os.environ.get("FORGE_SERVER"))
    p.add_argument("--environment")
    p.add_argument("--forge", default=os.environ.get("FORGE_BIN", "forge"))
    a = p.parse_args()
    token = a.token_file.read_text().strip() if a.token_file else os.environ.get("FORGE_TOKEN", "")
    if not token:
        sys.exit("no token: --token-file or FORGE_TOKEN")
    values = dict(v.split("=", 1) for v in a.var)
    sys.exit(deploy(a.template.read_text(), values, token, forge=a.forge, server=a.server, environment=a.environment))


if __name__ == "__main__":
    main()
