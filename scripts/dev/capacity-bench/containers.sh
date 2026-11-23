#!/usr/bin/env bash
# Test containers for the real node (alpine/socat image, busybox sh).
# Usage: containers.sh silent <n>                 n containers that print nothing
#        containers.sh logging <tag> <n> <secs>   n containers, 10 lines/s each for secs, then silent
#        containers.sh lines <tag>                lines the tag's containers printed (docker logs)
#        containers.sh rm                         removes every bench-* container
# Containers are named bench-<tag>-<i> and run through `sg docker`.
set -euo pipefail
IMG=${IMG:-alpine/socat:1.8.0.3}
d() { sg docker -c "docker $*"; }
case $1 in
silent)
  for i in $(seq 1 "$2"); do
    d run -d --name "bench-quiet-$i" --entrypoint sh "$IMG" -c "'exec sleep 2147483647'" > /dev/null
  done ;;
logging)
  TAG=$2 N=$3 SECS=$4
  for i in $(seq 1 "$N"); do
    d run -d --name "bench-$TAG-$i" --entrypoint sh "$IMG" -c "'s=0; while [ \$s -lt $SECS ]; do l=0; while [ \$l -lt 10 ]; do echo \"$TAG c$i s\$s l\$l GET /api/items/\$s status=200 bytes=512 took_ms=12\"; l=\$((l+1)); done; s=\$((s+1)); sleep 1; done; exec sleep 2147483647'" > /dev/null
  done ;;
lines)
  t=0
  for c in $(d ps -a --format "'{{.Names}}'" | grep "^bench-$2-"); do
    n=$(d logs "$c" 2>&1 | wc -l); t=$((t + n))
  done
  echo "$t" ;;
rm)
  ids=$(d ps -aq --filter name=^bench-)
  [ -n "$ids" ] && d rm -f $ids > /dev/null
  true ;;
esac
