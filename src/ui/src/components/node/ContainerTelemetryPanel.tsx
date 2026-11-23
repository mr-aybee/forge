import { Plus, Trash2 } from "lucide-react";
import { useState } from "react";

import { type ContainerTelemetrySettings, useContainerTelemetrySettings, useSaveContainerTelemetry } from "../../api/containerTelemetry";
import { errorText, useToast } from "../../app/toast";
import { Button, ErrorNote, IconButton, Input, Mono, Panel, Skeleton, Switch, UnsavedBar } from "../ui";

const MAX = 64;
const validEntry = (x: string) => x.length > 0 && x.length <= 256 && !/[\x00-\x1f]/.test(x);

/**
 * Node settings for container telemetry: collect every Docker container's
 * logs and metrics, minus the excluded ones (a container name or a
 * `key=value` label). Saved through the unsaved bar; needs server.manage.
 */
export function ContainerTelemetryPanel({ server, canEdit }: { server: string; canEdit: boolean }) {
  const q = useContainerTelemetrySettings(server);
  return (
    <Panel title="Container telemetry" description="Whether this node sends the logs and CPU, memory and network metrics of its Docker containers to the analytics database.">
      <div className="px-4 py-3">
        {q.isPending ? <Skeleton rows={2} /> : q.isError ? <ErrorNote error={q.error} /> : <Form key={JSON.stringify(q.data)} server={server} saved={q.data} canEdit={canEdit} />}
      </div>
    </Panel>
  );
}

function Form({ server, saved, canEdit }: { server: string; saved: ContainerTelemetrySettings; canEdit: boolean }) {
  const save = useSaveContainerTelemetry(server);
  const toast = useToast();
  const [form, setForm] = useState<ContainerTelemetrySettings>(saved);
  const [entry, setEntry] = useState("");
  const clean = { enabled: form.enabled, exclude: form.exclude.map((x) => x.trim()).filter(Boolean) };
  const dirty = JSON.stringify(clean) !== JSON.stringify(saved);
  const bad = clean.exclude.find((x) => !validEntry(x));
  const dupe = clean.exclude.find((x, i) => clean.exclude.indexOf(x) !== i);
  const problem = bad ? `“${bad}” is not a valid entry` : dupe ? `“${dupe}” is listed twice` : clean.exclude.length > MAX ? `At most ${MAX} entries` : undefined;
  const add = () => {
    const v = entry.trim();
    if (!v) return;
    setForm((f) => ({ ...f, exclude: [...f.exclude, v] }));
    setEntry("");
  };
  return (
    <div className="space-y-3">
      <Switch
        checked={form.enabled}
        disabled={!canEdit}
        title={canEdit ? undefined : "Changing it needs server.manage"}
        onChange={(enabled) => setForm((f) => ({ ...f, enabled }))}
        label={<span className="text-[13px]">Collect container logs and metrics</span>}
      />
      <div className="space-y-1.5">
        <p className="text-xs font-medium text-muted">Excluded containers</p>
        {form.exclude.length === 0 ? (
          <p className="text-xs text-faint">None: every container is collected.</p>
        ) : (
          <ul className="divide-y divide-line rounded-md border border-line">
            {form.exclude.map((x, i) => (
              // Index keys: entries are plain strings and may repeat while editing.
              <li key={i} className="flex min-w-0 items-center gap-2 px-2 py-1">
                <span className={`min-w-0 flex-1 truncate font-mono text-xs select-all ${validEntry(x.trim()) ? "" : "text-bad"}`} title={x}>
                  {x}
                </span>
                <span className="shrink-0 text-[10px] text-faint">{x.includes("=") ? "label" : "name"}</span>
                {canEdit && (
                  <IconButton type="button" label={`Remove ${x}`} className="size-7 hover:text-bad" onClick={() => setForm((f) => ({ ...f, exclude: f.exclude.filter((_, j) => j !== i) }))}>
                    <Trash2 className="size-3.5" />
                  </IconButton>
                )}
              </li>
            ))}
          </ul>
        )}
        {canEdit && (
          <form
            className="flex max-w-md items-center gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              add();
            }}
          >
            <Input className="h-7 py-0 font-mono text-xs" aria-label="Container name or label to exclude" placeholder="redis-cache or com.example.telemetry=off" value={entry} maxLength={256} onChange={(e) => setEntry(e.target.value)} />
            <Button type="submit" variant="ghost" disabled={!entry.trim() || form.exclude.length >= MAX}>
              <Plus className="size-4" /> Exclude
            </Button>
          </form>
        )}
        <p className="text-xs text-faint">
          A container name, or <Mono>key=value</Mono> to match a label. Excluded containers keep running; Forge only stops reading them. The node applies changes with its next heartbeat.
        </p>
      </div>
      <UnsavedBar
        dirty={dirty}
        summary={problem}
        saveDisabled={!!problem}
        saving={save.isPending}
        onDiscard={() => setForm(saved)}
        onSave={() =>
          save.mutate(clean, {
            onSuccess: () => toast(clean.enabled ? "Container telemetry saved" : "Container telemetry turned off"),
            onError: (e) => toast(errorText(e), "error"),
          })
        }
      />
    </div>
  );
}
