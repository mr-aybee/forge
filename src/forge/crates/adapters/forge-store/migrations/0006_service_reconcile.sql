-- D-177: the service reconciler.
-- A drained node takes no new placements; the reconciler moves its movable
-- services elsewhere. 0/1.
ALTER TABLE servers ADD COLUMN drained BIGINT NOT NULL DEFAULT 0;
-- What the instance should be doing: `running` or `stopped` (set by start,
-- restart, deploy and stop). The reconciler only restarts `running` ones.
ALTER TABLE service_instances ADD COLUMN desired TEXT NOT NULL DEFAULT 'running';
-- The release label of the last successful deploy; a container reporting
-- another release is `drifted`.
ALTER TABLE service_instances ADD COLUMN deployed_release TEXT;
-- The exit code of the instance's container when it last exited.
ALTER TABLE service_instances ADD COLUMN exit_code BIGINT;
-- Instances already stopped stay stopped: the reconciler must not start
-- what someone stopped before it existed.
UPDATE service_instances SET desired = 'stopped' WHERE state IN ('inactive', 'exited', 'stopped');
