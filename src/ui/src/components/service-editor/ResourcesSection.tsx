import type { ServiceInput } from "../../api/types";
import { Input, Mono, Select } from "../ui";
import { FormField, Grid, NumberInput, RowsEditor, SubBlock } from "./parts";
import { ULIMITS, ulimitOk } from "./sections";

/** CPU and memory limits per node (0 means no limit); containers also get ulimits. */
export function ResourcesSection({ value, set }: { value: ServiceInput; set: (p: Partial<ServiceInput>) => void }) {
  const used = new Set(value.ulimits.map((u) => u.name));
  const nextName = ULIMITS.find((n) => !used.has(n)) ?? "nofile";
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
      {value.runtime === "container" && (
        <SubBlock
          title="Ulimits"
          hint={
            <>
              Per-process limits, for example <Mono>nofile</Mono> 65536 / 65536. <Mono>-1</Mono> is unlimited; soft must not exceed hard.
            </>
          }
        >
          <RowsEditor
            label="Ulimit"
            rows={value.ulimits}
            onChange={(ulimits) => set({ ulimits })}
            blank={() => ({ name: nextName, soft: 1024, hard: 1024 })}
            addLabel="Add ulimit"
            max={ULIMITS.length}
            render={(u, setRow, i) => {
              const dup = value.ulimits.findIndex((x) => x.name === u.name) !== i;
              const bad = !ulimitOk(u);
              return (
                <>
                  <div className="w-32 shrink-0">
                    <Select aria-label={`Ulimit ${i + 1} name`} className={`h-7 py-0 font-mono text-xs ${dup ? "border-bad" : ""}`} value={u.name} onChange={(e) => setRow({ ...u, name: e.target.value })}>
                      {ULIMITS.map((n) => (
                        <option key={n} value={n}>
                          {n}
                        </option>
                      ))}
                    </Select>
                  </div>
                  <Input type="number" inputMode="numeric" aria-label={`Ulimit ${i + 1} soft`} aria-invalid={bad || undefined} className={`h-7 w-28 py-0 font-mono text-xs tabular-nums ${bad ? "border-bad" : ""}`} min={-1} value={Number.isFinite(u.soft) ? u.soft : ""} placeholder="soft" onChange={(e) => setRow({ ...u, soft: e.target.value === "" ? Number.NaN : Math.round(Number(e.target.value)) })} />
                  <span className="text-xs text-faint">/</span>
                  <Input type="number" inputMode="numeric" aria-label={`Ulimit ${i + 1} hard`} aria-invalid={bad || undefined} className={`h-7 w-28 py-0 font-mono text-xs tabular-nums ${bad ? "border-bad" : ""}`} min={-1} value={Number.isFinite(u.hard) ? u.hard : ""} placeholder="hard" onChange={(e) => setRow({ ...u, hard: e.target.value === "" ? Number.NaN : Math.round(Number(e.target.value)) })} />
                </>
              );
            }}
          />
        </SubBlock>
      )}
    </Grid>
  );
}
