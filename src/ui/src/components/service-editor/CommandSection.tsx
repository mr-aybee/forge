import type { ServiceInput } from "../../api/types";
import { Input, Select } from "../ui";
import { FormField, Grid, NumberInput } from "./parts";

/** Container: command and entrypoint overrides, restart policy and stop timeout. */
export function CommandSection({ value, set }: { value: ServiceInput; set: (p: Partial<ServiceInput>) => void }) {
  const stopBad = value.stop_timeout_secs < 0 || value.stop_timeout_secs > 3600;
  return (
    <Grid>
      <FormField label="Command" hint="Overrides the image's CMD (run with /bin/sh -c).">
        {(id) => <Input id={id} className="font-mono" value={value.command} onChange={(e) => set({ command: e.target.value })} placeholder="Image default" spellCheck={false} />}
      </FormField>
      <FormField label="Entrypoint" hint="Overrides the image's ENTRYPOINT (words separated by spaces).">
        {(id) => <Input id={id} className="font-mono" value={value.entrypoint} onChange={(e) => set({ entrypoint: e.target.value })} placeholder="Image default" spellCheck={false} />}
      </FormField>
      <FormField label="Restart policy">
        {(id) => (
          <Select id={id} value={value.restart ? "unless-stopped" : "no"} onChange={(e) => set({ restart: e.target.value !== "no" })}>
            <option value="unless-stopped">Unless stopped</option>
            <option value="no">Never</option>
          </Select>
        )}
      </FormField>
      <FormField label="Stop timeout (seconds)" error={stopBad ? "0 to 3600." : undefined} hint="Grace period before the container is killed. Empty: Docker's default (10 s).">
        {(id, invalid) => <NumberInput id={id} className="w-36" min={0} max={3600} value={value.stop_timeout_secs} invalid={invalid} placeholder="10" onChange={(n) => set({ stop_timeout_secs: Math.round(n) || 0 })} />}
      </FormField>
    </Grid>
  );
}
