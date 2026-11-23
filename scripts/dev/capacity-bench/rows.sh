#!/usr/bin/env bash
# Prints the stored rows: total container lines, distinct (server, ts, message) lines.
# Usage: rows.sh [extra SQL]   Environment: B.
set -euo pipefail
q() { curl -sf -u "forge:$(cat "$B/data/analytics/secret")" http://127.0.0.1:18124/ --data-binary "$1"; }
q "${1:-SELECT count() FROM forge_op_analytics.app_logs WHERE source = 'container' FORMAT TSV}"
