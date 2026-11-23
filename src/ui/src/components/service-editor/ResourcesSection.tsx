import type { ServiceInput } from "../../api/types";
import { FormField, Grid, NumberInput } from "./parts";

/** CPU and memory limits per node; 0 means no limit. */
export function ResourcesSection({ value, set }: { value: ServiceInput; set: (p: Partial<ServiceInput>) => void }) {
  return (
    <Grid>
      <FormField label="CPU (cores)" error={value.cpu_millis < 0 ? "Cannot be negative." : undefined} hint="Fractions are fine: 0.5 is half a core.">
        {(id, invalid) => (
          <NumberInput id={id} className="w-36" min={0} step={0.1} value={value.cpu_millis / 1000} invalid={invalid} placeholder="unlimited" onChange={(n) => set({ cpu_millis: Math.round(n * 1000) })} />
        )}
      </FormField>
      <FormField label="Memory (MiB)" error={value.memory_mb < 0 ? "Cannot be negative." : undefined} hint="Hard limit; the process is killed above it. 1024 MiB = 1 GiB.">
        {(id, invalid) => <NumberInput id={id} className="w-36" min={0} step={64} value={value.memory_mb} invalid={invalid} placeholder="unlimited" onChange={(n) => set({ memory_mb: Math.round(n) })} />}
      </FormField>
    </Grid>
  );
}
