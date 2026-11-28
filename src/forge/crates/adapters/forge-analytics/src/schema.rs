//! ClickHouse tables. Each migration is applied once, in order, and
//! recorded in `forge_schema`. Version 1 is the baseline; add changes as new
//! versions after it. `{node_days}`, `{top_days}`, `{rollup_days}` and `{log_days}`
//! are replaced by the retention when it is applied; TTLs are re-applied at
//! every start and on every runtime change (D-136), so a retention change
//! takes effect without a new migration.

/// `(version, statements)`.
pub const MIGRATIONS: &[(u32, &[&str])] = &[
    (
        1,
        &[
            "CREATE TABLE IF NOT EXISTS node_metrics_1m (
            server_id UUID,
            minute DateTime('UTC'),
            cpu_avg Float32,
            cpu_max Float32,
            mem_used UInt64,
            mem_total UInt64,
            swap_used UInt64,
            disk_used UInt64,
            disk_total UInt64,
            rx UInt64,
            tx UInt64,
            load1 Float32,
            samples UInt32
        ) ENGINE = ReplacingMergeTree
        PARTITION BY toYYYYMMDD(minute)
        ORDER BY (server_id, minute)
        TTL minute + INTERVAL {node_days} DAY",
            "CREATE TABLE IF NOT EXISTS node_metrics_1h (
            server_id UUID,
            hour DateTime('UTC'),
            cpu_avg AggregateFunction(avg, Float32),
            cpu_max AggregateFunction(max, Float32),
            mem_used AggregateFunction(avg, UInt64),
            mem_total AggregateFunction(max, UInt64),
            disk_used AggregateFunction(avg, UInt64),
            disk_total AggregateFunction(max, UInt64),
            rx AggregateFunction(avg, UInt64),
            tx AggregateFunction(avg, UInt64),
            load1 AggregateFunction(avg, Float32)
        ) ENGINE = AggregatingMergeTree
        PARTITION BY toYYYYMM(hour)
        ORDER BY (server_id, hour)
        TTL hour + INTERVAL {rollup_days} DAY",
            "CREATE MATERIALIZED VIEW IF NOT EXISTS node_metrics_1h_mv TO node_metrics_1h AS
        SELECT server_id, toStartOfHour(minute) AS hour,
            avgState(cpu_avg) AS cpu_avg, maxState(cpu_max) AS cpu_max,
            avgState(mem_used) AS mem_used, maxState(mem_total) AS mem_total,
            avgState(disk_used) AS disk_used, maxState(disk_total) AS disk_total,
            avgState(rx) AS rx, avgState(tx) AS tx, avgState(load1) AS load1
        FROM node_metrics_1m GROUP BY server_id, hour",
            "CREATE TABLE IF NOT EXISTS node_top_processes_1m (
            server_id UUID,
            minute DateTime('UTC'),
            pid UInt32,
            name LowCardinality(String),
            user LowCardinality(String),
            cpu Float32,
            mem UInt64,
            cmd String CODEC(ZSTD(3))
        ) ENGINE = ReplacingMergeTree
        PARTITION BY toYYYYMMDD(minute)
        ORDER BY (server_id, minute, pid)
        TTL minute + INTERVAL {node_days} DAY",
            // Logs (D-128): pushed by services (`source = ingest`) and collected
            // from every Docker container on a node (`source = container`).
            // Ordered by node and time; a skip index on `service_id` keeps a
            // service's stream fast. Retention counts from `ingest_ts`, so an
            // app with a wrong clock cannot keep data longer.
            "CREATE TABLE IF NOT EXISTS app_logs (
            service_id Nullable(UUID),
            ts DateTime64(3, 'UTC'),
            id UUID,
            node_ts Nullable(DateTime64(3, 'UTC')),
            ingest_ts DateTime64(3, 'UTC'),
            level LowCardinality(String),
            message String CODEC(ZSTD(3)),
            message_template String CODEC(ZSTD(3)),
            exception String CODEC(ZSTD(3)),
            trace_id String,
            span_id String,
            event_type UInt32,
            server_id Nullable(UUID),
            properties String CODEC(ZSTD(3)),
            source LowCardinality(String) DEFAULT 'ingest',
            container_id String DEFAULT '',
            container_name LowCardinality(String) DEFAULT '',
            stack LowCardinality(String) DEFAULT '',
            stream LowCardinality(String) DEFAULT '',
            INDEX message_tokens lower(message) TYPE tokenbf_v1(8192, 3, 0) GRANULARITY 4,
            INDEX service_idx service_id TYPE bloom_filter GRANULARITY 4,
            INDEX container_idx container_name TYPE bloom_filter GRANULARITY 4
        ) ENGINE = MergeTree
        PARTITION BY toYYYYMMDD(ingest_ts)
        ORDER BY (server_id, ts, id)
        TTL toDateTime(ingest_ts) + INTERVAL {log_days} DAY
        SETTINGS allow_nullable_key = 1",
            // Docker containers per node and minute; re-sent samples of one
            // minute replace each other.
            "CREATE TABLE IF NOT EXISTS container_metrics_1m (
            server_id UUID,
            container_name String,
            minute DateTime('UTC'),
            container_id String,
            image LowCardinality(String),
            service_id Nullable(UUID),
            stack LowCardinality(String),
            cpu Float32,
            mem_used UInt64,
            mem_limit UInt64,
            rx UInt64,
            tx UInt64,
            block_read UInt64,
            block_write UInt64,
            restarts UInt32
        ) ENGINE = ReplacingMergeTree
        PARTITION BY toYYYYMMDD(minute)
        ORDER BY (server_id, container_name, minute)
        TTL minute + INTERVAL {node_days} DAY",
            "CREATE TABLE IF NOT EXISTS container_metrics_1h (
            server_id UUID,
            container_name String,
            hour DateTime('UTC'),
            container_id AggregateFunction(argMax, String, DateTime('UTC')),
            image AggregateFunction(argMax, String, DateTime('UTC')),
            service_id AggregateFunction(argMax, Nullable(UUID), DateTime('UTC')),
            stack AggregateFunction(argMax, String, DateTime('UTC')),
            cpu_avg AggregateFunction(avg, Float32),
            cpu_max AggregateFunction(max, Float32),
            mem_used AggregateFunction(avg, UInt64),
            mem_limit AggregateFunction(max, UInt64),
            rx AggregateFunction(avg, UInt64),
            tx AggregateFunction(avg, UInt64),
            block_read AggregateFunction(avg, UInt64),
            block_write AggregateFunction(avg, UInt64),
            restarts AggregateFunction(max, UInt32)
        ) ENGINE = AggregatingMergeTree
        PARTITION BY toYYYYMM(hour)
        ORDER BY (server_id, container_name, hour)
        TTL hour + INTERVAL {rollup_days} DAY",
            "CREATE MATERIALIZED VIEW IF NOT EXISTS container_metrics_1h_mv TO container_metrics_1h AS
        SELECT server_id, container_name, toStartOfHour(minute) AS hour,
            argMaxState(container_id, minute) AS container_id, argMaxState(image, minute) AS image,
            argMaxState(service_id, minute) AS service_id, argMaxState(stack, minute) AS stack,
            avgState(cpu) AS cpu_avg, maxState(cpu) AS cpu_max,
            avgState(mem_used) AS mem_used, maxState(mem_limit) AS mem_limit,
            avgState(rx) AS rx, avgState(tx) AS tx,
            avgState(block_read) AS block_read, avgState(block_write) AS block_write,
            maxState(restarts) AS restarts
        FROM container_metrics_1m GROUP BY server_id, container_name, hour",
        ],
    ),
    // Spool files are inserted once per table (D-145): an insert sent again
    // with the same `insert_deduplication_token` is acknowledged without a
    // write. Plain MergeTree tables keep no deduplication log unless told
    // to; the rollup tables need one too, so a duplicate block does not
    // reach them through their views. 10,000 inserts is hours of resends.
    (
        2,
        &[
            "ALTER TABLE node_metrics_1m MODIFY SETTING non_replicated_deduplication_window = 10000",
            "ALTER TABLE node_metrics_1h MODIFY SETTING non_replicated_deduplication_window = 10000",
            "ALTER TABLE node_top_processes_1m MODIFY SETTING non_replicated_deduplication_window = 10000",
            "ALTER TABLE app_logs MODIFY SETTING non_replicated_deduplication_window = 10000",
            "ALTER TABLE container_metrics_1m MODIFY SETTING non_replicated_deduplication_window = 10000",
            "ALTER TABLE container_metrics_1h MODIFY SETTING non_replicated_deduplication_window = 10000",
        ],
    ),
];

/// TTLs re-applied at every start: `(table, column, retention key)`.
pub const TTLS: &[(&str, &str, &str)] = &[
    ("node_metrics_1m", "minute", "{node_days}"),
    ("node_top_processes_1m", "minute", "{top_days}"),
    ("node_metrics_1h", "hour", "{rollup_days}"),
    ("app_logs", "toDateTime(ingest_ts)", "{log_days}"),
    ("container_metrics_1m", "minute", "{node_days}"),
    ("container_metrics_1h", "hour", "{rollup_days}"),
];
