import { Link } from "@tanstack/react-router";

import type { HealthCheck, ServiceInput } from "../../api/types";
import { Input, Mono } from "../ui";
import { Check, FormField, Grid, NumberInput } from "./parts";

const DEFAULT_CHECK: HealthCheck = { command: "", interval_secs: 30, timeout_secs: 5, retries: 3, start_period_secs: 0 };

const TIMINGS = [
  ["interval_secs", "Every (s)"],
  ["timeout_secs", "Timeout (s)"],
  ["retries", "Retries"],
  ["start_period_secs", "Start period (s)"],
] as const;

/**
 * Health checks. Containers can set a Docker HEALTHCHECK here; Forge's own
 * startup, readiness and liveness checks are on the Health Checks tab.
 */
export function HealthSection({ value, set, serviceId }: { value: ServiceInput; set: (p: Partial<ServiceInput>) => void; serviceId?: string }) {
  const hc = value.healthcheck;
  const tab = serviceId ? (
    <Link to="/services/$serviceId" params={{ serviceId }} search={{ tab: "health" }} className="text-accent hover:underline">
      Health Checks
    </Link>
  ) : (
    <b>Health Checks</b>
  );
  if (value.runtime === "systemd") {
    return (
      <p className="text-xs text-muted">
        {value.listen_port ? (
          <>
            By default Forge connects to port <Mono>{value.listen_port}</Mono> on each node.{" "}
          </>
        ) : (
          <>Without a listening port, the default check only watches the systemd unit. </>
        )}
        Add HTTP, TCP or command checks for startup, readiness and liveness on the {tab} tab{serviceId ? "" : " after you create the service"}.
      </p>
    );
  }
  return (
    <div className="space-y-3">
      <Check checked={!!hc} onChange={(on) => set({ healthcheck: on ? DEFAULT_CHECK : null })} label="Docker health check" hint="Off: Docker uses the image's own HEALTHCHECK, if any." />
      {hc && (
        <Grid>
          <FormField wide label="Command" error={!hc.command.trim() ? "Enter a command, or turn the check off." : undefined} hint="Exit code 0 means healthy.">
            {(id, invalid) => (
              <Input
                id={id}
                aria-invalid={invalid || undefined}
                className={`font-mono ${invalid ? "border-bad" : ""}`}
                value={hc.command}
                onChange={(e) => set({ healthcheck: { ...hc, command: e.target.value } })}
                placeholder="wget -qO- http://localhost/health || exit 1"
                spellCheck={false}
              />
            )}
          </FormField>
          <div className="grid grid-cols-2 gap-3 sm:grid-cols-4 md:col-span-2">
            {TIMINGS.map(([k, label]) => (
              <FormField key={k} label={label}>
                {(id) => <NumberInput id={id} min={0} value={hc[k]} onChange={(n) => set({ healthcheck: { ...hc, [k]: Math.max(0, Math.round(n)) } })} />}
              </FormField>
            ))}
          </div>
        </Grid>
      )}
      <p className="text-xs text-faint">Forge's startup, readiness and liveness checks are on the {tab} tab.</p>
    </div>
  );
}
