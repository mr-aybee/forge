#!/usr/bin/env bash
# Prints engine profile counters (insert, select, merge time; rows) as TSV. Environment: B.
"$(dirname "$0")/rows.sh" "SELECT event, value FROM system.events WHERE event IN ('InsertQuery','AsyncInsertQuery','InsertedRows','SelectQuery','MergedRows','Merge','MergeTotalMilliseconds','MergesTimeMilliseconds','SelectQueryTimeMicroseconds','InsertQueryTimeMicroseconds','AsyncInsertBytes','RejectedInserts','DelayedInserts','QueryMemoryLimitExceeded','OSCPUVirtualTimeMicroseconds','UserTimeMicroseconds','FailedQuery','FailedInsertQuery') FORMAT TSV"
