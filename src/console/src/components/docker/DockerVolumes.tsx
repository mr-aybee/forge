import { Database, Eraser, Plus, Search, Trash2 } from "lucide-react";
import { useMemo, useState } from "react";

import { type DockerVolume, useDockerVolumes } from "../../api/dockerResources";
import { Badge, Button, Confirm, CopyIcon, Empty, Field, Input, Modal, RowMenu, type RowAction, Skeleton, Switch, Table, Td, ago, time, useRowMenu } from "../ui";
import { Check, DockerProblem, type DockerRunner, InspectDrawer, Spin, Toolbar } from "./shared";
import { CodeField } from "../CodeField";

/** `key=value` lines as labels; the first bad line as an error. */
export function parseLabels(text: string): { labels: Record<string, string>; error?: string } {
  const labels: Record<string, string> = {};
  for (const raw of text.split("\n")) {
    const line = raw.trim();
    if (!line) continue;
    const eq = line.indexOf("=");
    const k = (eq < 0 ? line : line.slice(0, eq)).trim();
    if (!k) return { labels, error: `"${line}" has no key` };
    if (k.startsWith("forge.")) return { labels, error: `"${k}": forge.* labels are reserved for Forge` };
    labels[k] = eq < 0 ? "" : line.slice(eq + 1).trim();
  }
  return { labels };
}

export function DockerVolumes({ server, runner }: { server: string; runner: DockerRunner }) {
  const q = useDockerVolumes(server);
  const [search, setSearch] = useState("");
  const [picked, setPicked] = useState<Set<string>>(() => new Set());
  const [dialog, setDialog] = useState<{ kind: "create" } | { kind: "prune" } | { kind: "bulk" } | { kind: "remove"; v: DockerVolume } | { kind: "inspect"; v: DockerVolume } | null>(null);
  const rm = useRowMenu();
  const all = useMemo(() => q.data ?? [], [q.data]);
  const rows = useMemo(() => {
    const s = search.trim().toLowerCase();
    return all.filter((v) => !s || `${v.name} ${v.driver} ${Object.entries(v.labels).flat().join(" ")}`.toLowerCase().includes(s));
  }, [all, search]);
  const selectable = rows.filter((v) => !v.managed);
  const sel = rows.filter((v) => picked.has(v.name));
  const close = () => setDialog(null);

  return (
    <div>
      <Toolbar>
        <label className="flex min-w-0 flex-1 items-center gap-2">
          <Search className="size-3.5 shrink-0 text-faint" />
          <input data-bare className="min-w-0 flex-1 bg-transparent text-[13px] outline-none" placeholder="Filter by name, driver or label" aria-label="Filter volumes" value={search} onChange={(e) => setSearch(e.target.value)} />
        </label>
        {sel.length > 0 && (
          <Button variant="danger" disabled={runner.bulk} onClick={() => setDialog({ kind: "bulk" })}>
            <Spin on={runner.bulk}>
              <Trash2 className="size-4" />
            </Spin>{" "}
            Remove {sel.length}
          </Button>
        )}
        <Button onClick={() => setDialog({ kind: "prune" })}>
          <Eraser className="size-4" /> Prune
        </Button>
        <Button variant="primary" onClick={() => setDialog({ kind: "create" })}>
          <Plus className="size-4" /> Create volume
        </Button>
      </Toolbar>
      {q.error ? (
        <DockerProblem error={q.error} server={server} />
      ) : q.isLoading ? (
        <Skeleton rows={4} />
      ) : rows.length === 0 ? (
        <Empty icon={<Database className="size-5" />} title={all.length ? "No volume matches" : "No volumes"}>
          {all.length ? "Change the search." : "Create one to keep data across container rebuilds."}
        </Empty>
      ) : (
        <Table
          head={[
            <Check
              key="all"
              label="Select every removable volume shown"
              checked={selectable.length > 0 && selectable.every((v) => picked.has(v.name))}
              disabled={selectable.length === 0}
              onChange={(on) => setPicked(on ? new Set(selectable.map((v) => v.name)) : new Set())}
            />,
            "Name",
            "Driver",
            "Mountpoint",
            "Labels",
            "Created",
            "",
          ]}
        >
          {rows.map((v) => {
            const actions: RowAction[] = [
              { label: "Inspect", icon: Search, tone: "accent", onSelect: () => setDialog({ kind: "inspect", v }) },
              { label: "Remove…", icon: Trash2, danger: true, disabled: runner.pending, hint: "Another Docker action is running", onSelect: () => setDialog({ kind: "remove", v }) },
            ];
            const lbl = Object.entries(v.labels);
            return (
              <tr key={v.name} className="hover:bg-panel-2/60" {...rm.rowProps(actions, `Actions for ${v.name}`)}>
                <Td className="w-8">
                  <Check label={`Select ${v.name}`} checked={picked.has(v.name)} disabled={v.managed} title={v.managed ? "Forge manages this volume; remove it on its own" : undefined} onChange={(on) => setPicked((p) => { const n = new Set(p); if (on) n.add(v.name); else n.delete(v.name); return n; })} />
                </Td>
                <Td className="max-w-[280px]">
                  <div className="flex min-w-0 items-center gap-1.5">
                    <span className="truncate font-mono text-[12px] select-all" title={v.name}>
                      {v.name}
                    </span>
                    <CopyIcon text={v.name} label="Copy volume name" />
                    {v.managed && <Badge tone="accent">Forge</Badge>}
                  </div>
                </Td>
                <Td className="text-xs text-muted">
                  {v.driver}
                  {v.scope && v.scope !== "local" && <span className="text-faint"> · {v.scope}</span>}
                </Td>
                <Td className="max-w-[260px] truncate font-mono text-[11px] text-muted" title={v.mountpoint}>
                  {v.mountpoint ?? "—"}
                </Td>
                <Td className="max-w-[220px]">
                  {lbl.length ? (
                    <span className="block truncate font-mono text-[11px] text-muted" title={lbl.map(([k, x]) => `${k}=${x}`).join("\n")}>
                      {lbl.map(([k, x]) => `${k}=${x}`).join(", ")}
                    </span>
                  ) : (
                    <span className="text-faint">—</span>
                  )}
                </Td>
                <Td className="text-xs whitespace-nowrap text-muted" title={time(v.created)}>
                  {v.created ? ago(v.created) : "—"}
                </Td>
                <Td className="text-right">
                  <RowMenu actions={actions} label={`Actions for ${v.name}`} />
                </Td>
              </tr>
            );
          })}
        </Table>
      )}
      {rm.menu}

      {dialog?.kind === "inspect" && <InspectDrawer server={server} op={{ op: "volume_inspect", name: dialog.v.name }} title={`Inspect · ${dialog.v.name}`} onClose={close} />}
      {dialog?.kind === "create" && <CreateVolume runner={runner} onClose={close} />}
      {dialog?.kind === "remove" && <RemoveVolume v={dialog.v} runner={runner} onClose={close} />}
      {dialog?.kind === "prune" && (
        <Confirm
          open
          onOpenChange={(o) => !o && close()}
          title="Prune volumes?"
          busy={runner.pending}
          body={
            <div className="space-y-2">
              <p>
                Docker deletes every <b>anonymous</b> volume no container uses, with its data. Named volumes and volumes of stopped containers stay. Forge's volumes are kept.
              </p>
              <p className="text-warn">This is host-wide: it covers every unused anonymous volume on this node, including those of containers Forge did not create.</p>
              <p className="text-warn">The data in those volumes cannot be recovered. The prune runs as a job.</p>
            </div>
          }
          confirmLabel="Prune volumes"
          onConfirm={() => void runner.run({ op: "volume_prune" }, "Volume prune").then((ok) => ok && close())}
        />
      )}
      {dialog?.kind === "bulk" && (
        <Confirm
          open
          onOpenChange={(o) => !o && close()}
          title={`Remove ${sel.length} volumes?`}
          body={
            <>
              <p>Docker deletes these volumes and their data. A volume a container still uses is refused and stays.</p>
              <ul className="mt-2 max-h-40 overflow-auto font-mono text-[11px]">
                {sel.map((v) => (
                  <li key={v.name} className="truncate" title={v.name}>
                    {v.name}
                  </li>
                ))}
              </ul>
            </>
          }
          confirmLabel={`Remove ${sel.length}`}
          onConfirm={() => {
            const ops = sel.map((v) => ({ op: "volume_remove" as const, name: v.name, force: false }));
            close();
            void runner.runMany(ops, "volumes", () => setPicked(new Set()));
          }}
        />
      )}
    </div>
  );
}

function CreateVolume({ runner, onClose }: { runner: DockerRunner; onClose: () => void }) {
  const [form, setForm] = useState({ name: "", labels: "" });
  const parsed = parseLabels(form.labels);
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title="Create volume" description="A named volume with the local driver on this node.">
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          if (parsed.error) return;
          const name = form.name.trim();
          void runner.run({ op: "volume_create", name, labels: parsed.labels }, `Created volume ${name}`).then((ok) => ok && onClose());
        }}
      >
        <Field label="Name">
          <Input autoFocus required pattern="[A-Za-z0-9][A-Za-z0-9_.\-]*" className="font-mono" value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="app-data" />
        </Field>
        <Field label="Labels" hint={parsed.error ? <span className="text-bad">{parsed.error}</span> : "One key=value per line; optional."}>
          <CodeField lang="properties" value={form.labels} onChange={(labels) => setForm({ ...form, labels })} placeholder="team=payments" />
        </Field>
        <div className="flex justify-end gap-2">
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={runner.pending || !form.name.trim() || !!parsed.error}>
            <Spin on={runner.pending}>
              <Plus className="size-4" />
            </Spin>{" "}
            Create
          </Button>
        </div>
      </form>
    </Modal>
  );
}

function RemoveVolume({ v, runner, onClose }: { v: DockerVolume; runner: DockerRunner; onClose: () => void }) {
  const [force, setForce] = useState(false);
  return (
    <Confirm
      open
      onOpenChange={(o) => !o && onClose()}
      title={`Remove volume ${v.name}?`}
      busy={runner.pending}
      body={
        <div className="space-y-3">
          <p>Docker deletes the volume and all data in it. A volume a container uses is refused unless you force it.</p>
          {v.managed && <p className="text-warn">Forge created this volume for an app or stack; removing it loses that app's data.</p>}
          <Switch checked={force} onChange={setForce} label="Force" />
        </div>
      }
      confirmLabel={force ? "Force remove" : "Remove"}
      onConfirm={() => void runner.run({ op: "volume_remove", name: v.name, force }, `Removed volume ${v.name}`).then((ok) => ok && onClose())}
    />
  );
}
