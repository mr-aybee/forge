import type { ServiceInput } from "../../api/types";
import { Input } from "../ui";
import { Check, FormField, Grid, NumberInput, RestartFields } from "./parts";

/** Container: restart policy, user, host name, init and stop timeout. */
export function RunSection({ value, set }: { value: ServiceInput; set: (p: Partial<ServiceInput>) => void }) {
  const stopBad = value.stop_timeout_secs < 0 || value.stop_timeout_secs > 3600;
  return (
    <Grid>
      {value.swarm ? (
        <p className="text-xs text-faint md:col-span-2">Restarts are managed by the swarm: set the restart condition under Swarm.</p>
      ) : (
        <RestartFields value={value} set={set} />
      )}
      <FormField label="Stop timeout (seconds)" error={stopBad ? "0 to 3600." : undefined} hint="Grace period before the container is killed. Empty: Docker's default (10 s).">
        {(id, invalid) => <NumberInput id={id} className="w-36" min={0} max={3600} value={value.stop_timeout_secs} invalid={invalid} placeholder="10" onChange={(n) => set({ stop_timeout_secs: Math.round(n) || 0 })} />}
      </FormField>
      <FormField label="Run as user" hint="name, uid or uid:gid. Default: the image's user.">
        {(id) => <Input id={id} className="font-mono" value={value.user} onChange={(e) => set({ user: e.target.value.trim() })} placeholder="1000:1000" spellCheck={false} />}
      </FormField>
      <FormField label="Host name" hint="Inside the container. Default: the container id.">
        {(id) => <Input id={id} className="font-mono" value={value.hostname} onChange={(e) => set({ hostname: e.target.value.trim() })} placeholder="api" spellCheck={false} />}
      </FormField>
      <div className="md:col-span-2">
        <Check checked={value.init} onChange={(init) => set({ init })} label="Run an init process" hint="tini as PID 1: reaps zombie processes and forwards signals. Useful for shells and process managers." />
      </div>
    </Grid>
  );
}
