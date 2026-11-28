import { Code2, LayoutGrid } from "lucide-react";
import { useState } from "react";

import type { ServiceInput } from "../../api/types";
import { Mono } from "../ui";
import { FormField, Grid, NumberInput } from "./parts";
import { fromComposeYaml, toComposeYaml } from "./yaml";
import { CodeField } from "../CodeField";

/** Form or YAML view of the container's Advanced card; lives in the card header. */
export function YamlToggle({ yaml, onChange }: { yaml: boolean; onChange: (y: boolean) => void }) {
  return (
    <div className="flex rounded-md border border-line p-0.5 text-xs" role="tablist" aria-label="Advanced view">
      {(
        [
          [false, LayoutGrid, "Form"],
          [true, Code2, "YAML"],
        ] as const
      ).map(([y, Icon, label]) => (
        <button key={label} type="button" role="tab" aria-selected={yaml === y} onClick={() => onChange(y)} className={`inline-flex items-center gap-1 rounded px-2 py-0.5 ${yaml === y ? "bg-accent-soft text-accent" : "text-muted hover:text-fg"}`}>
          <Icon className="size-3.5" /> {label}
        </button>
      ))}
    </div>
  );
}

const labelsText = (l: [string, string][]) => l.map(([k, v]) => `${k}=${v}`).join("\n");
const parseLabels = (t: string): [string, string][] =>
  t
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean)
    .map((l) => {
      const [k, v] = l.split(/=(.*)/s);
      return [k ?? "", v ?? ""] as [string, string];
    });

/** Advanced options. Containers also get the whole definition as compose YAML. */
export function AdvancedSection({ value, set, onReplace, yaml, facts }: { value: ServiceInput; set: (p: Partial<ServiceInput>) => void; onReplace: (v: ServiceInput) => void; yaml: boolean; facts: { unit: string; user: string } }) {
  if (value.runtime === "systemd") {
    return (
      <dl className="grid grid-cols-[150px_minmax(0,1fr)] gap-x-3 gap-y-1.5 text-[13px]">
        <dt className="text-muted">systemd unit</dt>
        <dd className="min-w-0 truncate select-all" title={facts.unit}>
          <Mono>{facts.unit}</Mono>
        </dd>
        <dt className="text-muted">Runs as</dt>
        <dd className="min-w-0 truncate">
          <Mono>{facts.user}</Mono> <span className="text-xs text-muted">· its own system user, never root</span>
        </dd>
        <dt className="text-muted">Not available</dt>
        <dd className="text-xs text-muted">Image, network, storage, ulimits, security, labels and log size apply to containers only. Restart policy and limits are in Start command and Limits.</dd>
      </dl>
    );
  }
  if (yaml) return <YamlEditor value={value} onChange={onReplace} />;
  return (
    <Grid>
      <FormField label="Log rotation (MiB)" error={value.log_max_mb < 0 ? "Cannot be negative." : undefined} hint="Rotate the container log at this size (3 files kept). Empty: Docker's default.">
        {(id, invalid) => <NumberInput id={id} className="w-36" min={0} value={value.log_max_mb} invalid={invalid} placeholder="default" onChange={(n) => set({ log_max_mb: Math.round(n) })} />}
      </FormField>
      <LabelsField value={value.labels} onChange={(labels) => set({ labels })} />
    </Grid>
  );
}

/** Labels as `key=value` lines; applied when the field loses focus. */
function LabelsField({ value, onChange }: { value: [string, string][]; onChange: (v: [string, string][]) => void }) {
  const [text, setText] = useState(() => labelsText(value));
  return (
    <FormField label="Labels" hint="key=value per line. forge.* is reserved.">
      {(id) => <CodeField id={id} lang="properties" maxLines={10} value={text} onChange={setText} onBlur={() => onChange(parseLabels(text))} placeholder="team=payments" />}
    </FormField>
  );
}

function YamlEditor({ value, onChange }: { value: ServiceInput; onChange: (v: ServiceInput) => void }) {
  const [text, setText] = useState(() => toComposeYaml(value));
  const [error, setError] = useState("");
  return (
    <div className="space-y-2">
      <CodeField
        aria-label="Container definition as compose YAML"
        lang="yaml"
        invalid={!!error}
        minLines={14}
        maxLines={40}
        value={text}
        onChange={(t) => {
          setText(t);
          try {
            onChange(fromComposeYaml(t, value));
            setError("");
          } catch (err) {
            setError(err instanceof Error ? err.message : String(err));
          }
        }}
      />
      {error ? (
        <p className="text-xs text-bad" role="alert">
          {error}
        </p>
      ) : (
        <p className="text-xs text-faint">docker-compose service syntax; changes apply to every card as you type. Secrets show as &lt;sealed&gt; and are kept.</p>
      )}
    </div>
  );
}
