#!/usr/bin/env bash
# DuckDB store: prints "<container lines> <distinct lines>" (tab-separated).
# Stop forge-server first (by exact pid): DuckDB locks its files.
# Environment: B; DUCK_COUNT (the duck_count example binary).
set -euo pipefail
"${DUCK_COUNT:-src/forge/target/release/examples/duck_count}" "$B/data/analytics/duckdb/logs"
