D=$(dirname "$0"); OUT=${OUT:-/tmp}
s(){ sg docker -c "docker stats --no-stream --format '{{.CPUPerc}} {{.MemUsage}}' seq-bench" | sed "s/^/$1 /"; }
for i in 1 2 3 4; do s idle; sleep 5; done
python3 -I $D/load.py 2000 180 > $OUT/seq-load.out 2>&1 & L=$!
while kill -0 $L 2>/dev/null; do s load; sleep 5; done
cat $OUT/seq-load.out
for i in $(seq 1 12); do s after; sleep 5; done
