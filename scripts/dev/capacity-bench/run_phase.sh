#!/usr/bin/env bash
# One benchmark phase: fleet-sim soak + console readers + 5 s sampler.
# Usage: run_phase.sh <name> <agents> <seconds> <lines/s per node> [users] [containers] [settle] [logging containers]
#   IDLE_SECS / POST_SECS (default 0): sample that long before the load and after the settle.
# Environment: B (benchmark dir with forge.toml, data/, admin-password.txt),
#   API (http://127.0.0.1:56130), AGENT_URL (https://127.0.0.1:56131), SIM (fleet-sim binary).
set -euo pipefail
NAME=$1 AGENTS=$2 SECS=$3 RATE=$4 USERS=${5:-4} CONTAINERS=${6:-5} SETTLE=${7:-120} LOGGING=${8:-0}
HERE=$(cd "$(dirname "$0")" && pwd)
OUT=$B/results/$NAME; mkdir -p "$OUT"
LOGSTART=$(stat -c %s "$B/server.log")
ENGLOG=$(ls "$B"/data/analytics/log/*err*.log 2>/dev/null | head -1 || true)
ENGSTART=$( [ -n "$ENGLOG" ] && stat -c %s "$ENGLOG" || echo 0)
SPID=$(pgrep -x forge-server)
"$HERE/rows.sh" > "$OUT/rows_before" || true  # ClickHouse only; DuckDB: rows_duck.sh after stopping
"$HERE/events.sh" > "$OUT/events_before" || true
python3 -I "$HERE/sample.py" "$B" "$OUT/sample.jsonl" &
SAMPLER=$!
sleep "${IDLE_SECS:-0}"
"$SIM" --target "$API" --target-agent-url "$AGENT_URL" --password-file "$B/admin-password.txt" \
  --state-dir "$B/nodes" --agents "$AGENTS" --duration-secs "$SECS" --report-secs 10 \
  --log-lines-per-sec "$RATE" --containers "$CONTAINERS" --logging-containers "$LOGGING" --server-pid "$SPID" \
  --settle-secs "$SETTLE" --out "$OUT/sim-total.json" 2> "$OUT/sim.jsonl" > /dev/null &
SIMPID=$!
until grep -q ready_secs "$OUT/sim.jsonl" 2>/dev/null; do
  kill -0 $SIMPID 2>/dev/null || { echo "fleet-sim exited early"; tail "$OUT/sim.jsonl"; kill $SAMPLER; exit 1; }
  sleep 1
done
date +%s > "$OUT/ready_t"
python3 -I "$HERE/readers.py" "$API" "$B/admin-password.txt" "$USERS" "$SECS" "$OUT/readers.jsonl" &
READERS=$!
wait $SIMPID || echo "fleet-sim failed: $?"
wait $READERS || true
sleep 5
"$HERE/rows.sh" > "$OUT/rows_after" || true
sleep "${POST_SECS:-0}"
"$HERE/events.sh" > "$OUT/events_after" || true
kill $SAMPLER || true
tail -c +$((LOGSTART + 1)) "$B/server.log" > "$OUT/server.log"
[ -n "$ENGLOG" ] && tail -c +$((ENGSTART + 1)) "$ENGLOG" > "$OUT/engine.err.log" || true
python3 -I "$HERE/summarize.py" "$OUT" "$SECS" | tee "$OUT/summary.json"
