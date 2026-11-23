import { useQueryClient } from "@tanstack/react-query";
import { Copy, Eye, EyeOff, KeyRound, Link2, Pencil, Plus, RefreshCw, Trash2, Wand2, X } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { api, ApiError, post } from "../../api/client";
import { type VaultItem, useVaultItems } from "../../api/vault";
import { useAllEnvironments,useServices, useClusters, useServers } from "../../api/hooks";
import { nodeName } from "../../api/types";
import { errorText, useToast } from "../../app/toast";
import { Badge, Button, Confirm, Empty, Field, Input, Modal, Panel, RowMenu, type RowAction, SearchInput, Select, Skeleton, Table, Td, TextArea, ago, useRowMenu } from "../ui";
import { COPY_BLOCKED, copyText } from "../../lib/clipboard";

export type { VaultItem };

const REVEAL_SECONDS = 30;
const KIND_LABEL = { node: "Node", service: "Service", cluster: "Cluster" } as const;

/** A strong random password from the browser's CSPRNG. */
export function generatePassword(length = 24) {
  const chars = "ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789!@#$%^&*-_=+";
  const out: string[] = [];
  const buf = new Uint32Array(1);
  while (out.length < length) {
    crypto.getRandomValues(buf);
    // Rejection sampling keeps the distribution uniform.
    if (buf[0]! < Math.floor(2 ** 32 / chars.length) * chars.length) out.push(chars[buf[0]! % chars.length]!);
  }
  return out.join("");
}

/** Credentials kept for people: platform admins only; passwords revealed on demand. */
export function Credentials({ linkFilter }: { linkFilter?: { kind: string; id: string } }) {
  const qc = useQueryClient();
  const toast = useToast();
  const items = useVaultItems();
  const [q, setQ] = useState("");
  const [tag, setTag] = useState("");
  const [kind, setKind] = useState("");
  const [editing, setEditing] = useState<VaultItem | "new">();
  const [deleting, setDeleting] = useState<VaultItem>();
  const [revealed, setRevealed] = useState<Record<string, { password: string; until: number }>>({});
  const [, tick] = useState(0);
  const rm = useRowMenu();

  // Hide revealed passwords again after REVEAL_SECONDS; drop them from memory.
  useEffect(() => {
    if (!Object.keys(revealed).length) return;
    const t = setInterval(() => {
      const now = Date.now();
      setRevealed((r) => Object.fromEntries(Object.entries(r).filter(([, v]) => v.until > now)));
      tick((n) => n + 1);
    }, 1000);
    return () => clearInterval(t);
  }, [revealed]);
  useEffect(() => () => setRevealed({}), []);

  const fetchPassword = async (i: VaultItem) => (await post<{ password: string }>(`/api/v1/vault/items/${i.id}/reveal`, {})).password;
  const reveal = async (i: VaultItem) => {
    try {
      const password = await fetchPassword(i);
      setRevealed((r) => ({ ...r, [i.id]: { password, until: Date.now() + REVEAL_SECONDS * 1000 } }));
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const copyPassword = async (i: VaultItem) => {
    try {
      const pw = revealed[i.id]?.password ?? (await fetchPassword(i));
      if (await copyText(pw)) toast("Password copied. Clear your clipboard when you are done.");
      else toast(COPY_BLOCKED, "error");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const copy = (text: string, what: string) => void copyText(text).then((ok) => (ok ? toast(`${what} copied`) : toast(COPY_BLOCKED, "error")));

  const forbidden = items.error instanceof ApiError && items.error.status === 403;
  const all = items.data ?? [];
  const tags = [...new Set(all.flatMap((i) => i.tags))].sort();
  const rows = all.filter(
    (i) =>
      (!tag || i.tags.includes(tag)) &&
      (!kind || i.links.some((l) => l.kind === kind)) &&
      (!linkFilter || i.links.some((l) => l.kind === linkFilter.kind && l.id === linkFilter.id)) &&
      `${i.name} ${i.username} ${i.url} ${i.tags.join(" ")} ${i.links.map((l) => l.name).join(" ")}`.toLowerCase().includes(q.toLowerCase()),
  );

  if (forbidden)
    return (
      <Panel>
        <Empty icon={<KeyRound className="size-5" />} title="Platform administrators only">
          The credential vault is limited to platform administrators.
        </Empty>
      </Panel>
    );

  return (
    <>
      <div className="mb-4 flex flex-wrap items-center gap-2 rounded-lg border border-line bg-panel p-3 shadow-[var(--shadow-card)] lg:flex-nowrap">
        <div className="min-w-[160px] flex-1 [&_input]:w-full [&_label]:w-full">
          <SearchInput value={q} onChange={setQ} placeholder="Search name, user, URL, tag or link" />
        </div>
        <Select className="!w-40 shrink-0" value={tag} onChange={(e) => setTag(e.target.value)} aria-label="Filter by tag">
          <option value="">All tags</option>
          {tags.map((t) => (
            <option key={t} value={t}>
              {t}
            </option>
          ))}
        </Select>
        <Select className="!w-40 shrink-0" value={kind} onChange={(e) => setKind(e.target.value)} aria-label="Filter by link">
          <option value="">Any link</option>
          <option value="node">Linked to a node</option>
          <option value="service">Linked to a service</option>
          <option value="cluster">Linked to a cluster</option>
        </Select>
        <Button variant="primary" className="shrink-0" onClick={() => setEditing("new")}>
          <Plus className="size-4" /> New credential
        </Button>
      </div>

      <Panel title={`${rows.length} credential${rows.length === 1 ? "" : "s"}`}>
        {items.isLoading ? (
          <Skeleton />
        ) : rows.length === 0 ? (
          <Empty icon={<KeyRound className="size-5" />} title={all.length ? "Nothing matches" : "No credentials yet"}>
            {all.length ? "Change the filters above." : "Store accounts for nodes, services and clusters. Passwords are encrypted and shown only to platform administrators after an MFA check."}
          </Empty>
        ) : (
          <Table head={["Name", "Username", "Password", "Tags", "Linked to", "Changed", ""]}>
            {rows.map((i) => {
              const r = revealed[i.id];
              const left = r ? Math.max(0, Math.ceil((r.until - Date.now()) / 1000)) : 0;
              const actions: RowAction[] = [
                { label: r ? "Hide password" : "Reveal password", icon: r ? EyeOff : Eye, onSelect: () => (r ? setRevealed((x) => Object.fromEntries(Object.entries(x).filter(([k]) => k !== i.id))) : void reveal(i)) },
                { label: "Copy password", icon: Copy, onSelect: () => void copyPassword(i) },
                { label: "Copy username", icon: Copy, hidden: !i.username, onSelect: () => copy(i.username, "Username") },
                { label: "Edit", icon: Pencil, onSelect: () => setEditing(i) },
                { label: "Delete", icon: Trash2, danger: true, onSelect: () => setDeleting(i) },
              ];
              return (
                <tr key={i.id} {...rm.rowProps(actions, `Actions for ${i.name}`)}>
                  <Td>
                    <div className="font-medium">{i.name}</div>
                    {i.url && <div className="max-w-[240px] truncate font-mono text-[11px] text-faint">{i.url}</div>}
                  </Td>
                  <Td>
                    {i.username ? (
                      <button onClick={() => copy(i.username, "Username")} className="inline-flex items-center gap-1 font-mono text-xs hover:text-accent" title="Copy username">
                        {i.username} <Copy className="size-3 opacity-60" />
                      </button>
                    ) : (
                      <span className="text-faint">—</span>
                    )}
                  </Td>
                  <Td>
                    {r ? (
                      <span className="inline-flex items-center gap-1.5">
                        <code className="rounded bg-panel-2 px-1.5 py-0.5 text-xs select-all">{r.password}</code>
                        <span className="text-[10px] text-faint tabular-nums">{left}s</span>
                        <button onClick={() => setRevealed((x) => Object.fromEntries(Object.entries(x).filter(([k]) => k !== i.id)))} title="Hide" className="text-muted hover:text-fg">
                          <EyeOff className="size-3.5" />
                        </button>
                      </span>
                    ) : (
                      <button onClick={() => void reveal(i)} className="inline-flex items-center gap-1.5 text-muted hover:text-accent" title="Reveal (MFA)">
                        <span className="tracking-widest">••••••••</span> <Eye className="size-3.5" />
                      </button>
                    )}
                  </Td>
                  <Td>
                    <div className="flex flex-wrap gap-1">
                      {i.tags.map((t) => (
                        <Badge key={t}>{t}</Badge>
                      ))}
                    </div>
                  </Td>
                  <Td>
                    <div className="flex flex-wrap gap-1">
                      {i.links.map((l) => (
                        <Badge key={`${l.kind}:${l.id}`} tone="accent">
                          {KIND_LABEL[l.kind]}: {l.name}
                        </Badge>
                      ))}
                    </div>
                  </Td>
                  <Td className="text-muted" title={`Password changed ${ago(i.rotated_at)}`}>
                    {ago(i.updated_at)}
                  </Td>
                  <Td className="text-right">
                    <RowMenu actions={actions} label={`Actions for ${i.name}`} />
                  </Td>
                </tr>
              );
            })}
          </Table>
        )}
      </Panel>
      {rm.menu}

      {editing && (
        <ItemEditor
          item={editing === "new" ? undefined : editing}
          preset={linkFilter}
          onClose={() => setEditing(undefined)}
          onSaved={() => {
            setEditing(undefined);
            void qc.invalidateQueries({ queryKey: ["vault-items"] });
          }}
        />
      )}
      <Confirm
        open={!!deleting}
        onOpenChange={(o) => !o && setDeleting(undefined)}
        title={`Delete “${deleting?.name}”?`}
        body="The password is destroyed. This cannot be undone."
        confirmLabel="Delete"
        onConfirm={async () => {
          if (!deleting) return;
          try {
            await api("DELETE", `/api/v1/vault/items/${deleting.id}`);
            toast("Credential deleted");
            setDeleting(undefined);
            void qc.invalidateQueries({ queryKey: ["vault-items"] });
          } catch (e) {
            toast(errorText(e), "error");
          }
        }}
      />
    </>
  );
}

function ItemEditor({ item, preset, onClose, onSaved }: { item?: VaultItem; preset?: { kind: string; id: string }; onClose: () => void; onSaved: () => void }) {
  const toast = useToast();
  const nodes = useServers().data ?? [];
  const clusters = useClusters().data ?? [];
  const apps = useServices().data ?? [];
  const environments = useAllEnvironments().data ?? [];
  const [f, setF] = useState({
    name: item?.name ?? "",
    username: item?.username ?? "",
    password: "",
    url: item?.url ?? "",
    notes: item?.notes ?? "",
    tags: item?.tags ?? [],
    links: item?.links.map(({ kind, id }) => ({ kind, id })) ?? (preset ? [preset] : []),
  });
  const [tagInput, setTagInput] = useState("");
  const [show, setShow] = useState(false);
  const [busy, setBusy] = useState(false);
  const [pick, setPick] = useState("");

  const targets = useMemo(
    () => [
      ...clusters.map((c) => ({ kind: "cluster", id: c.id, name: c.name })),
      ...nodes.filter((n) => n.status !== "revoked").map((n) => ({ kind: "node", id: n.id, name: nodeName(n) })),
      ...apps.map((a) => ({ kind: "service", id: a.id, name: `${a.name}.${environments.find((n) => n.id === a.environment_id)?.name ?? ""}` })),
    ],
    [clusters, nodes, apps, environments],
  );
  const nameOf = (k: string, id: string) => targets.find((t) => t.kind === k && t.id === id)?.name ?? id.slice(0, 8);
  const addTag = () => {
    const t = tagInput.trim().toLowerCase().replace(/,$/, "");
    if (t && !f.tags.includes(t)) setF({ ...f, tags: [...f.tags, t] });
    setTagInput("");
  };

  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={item ? `Edit “${item.name}”` : "New credential"} description="Only platform administrators can see this vault. The password is encrypted; everything else is searchable metadata." wide>
      <form
        className="space-y-4"
        onSubmit={async (e) => {
          e.preventDefault();
          setBusy(true);
          const body = { ...f, password: f.password || undefined };
          try {
            if (item) await api("PUT", `/api/v1/vault/items/${item.id}`, body);
            else await post("/api/v1/vault/items", body);
            toast(item ? "Credential updated" : "Credential saved");
            onSaved();
          } catch (err) {
            toast(errorText(err), "error");
          } finally {
            setBusy(false);
          }
        }}
      >
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="Name">
            <Input required autoFocus maxLength={100} value={f.name} onChange={(e) => setF({ ...f, name: e.target.value })} placeholder="Postgres root on db-1" />
          </Field>
          <Field label="Username">
            <Input maxLength={200} autoComplete="off" value={f.username} onChange={(e) => setF({ ...f, username: e.target.value })} placeholder="root" />
          </Field>
        </div>
        <Field label={item ? "New password" : "Password"} hint={item ? "Leave empty to keep the current password. Entering one rotates it." : undefined}>
          <div className="flex gap-2">
            <Input
              type={show ? "text" : "password"}
              autoComplete="new-password"
              required={!item}
              maxLength={4096}
              className="font-mono"
              value={f.password}
              onChange={(e) => setF({ ...f, password: e.target.value })}
            />
            <Button type="button" variant="ghost" onClick={() => setShow((s) => !s)} title={show ? "Hide" : "Show"}>
              {show ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
            </Button>
            <Button
              type="button"
              onClick={() => {
                setF({ ...f, password: generatePassword() });
                setShow(true);
              }}
            >
              <Wand2 className="size-4" /> Generate
            </Button>
          </div>
        </Field>
        <Field label="URL" hint="Optional: https://, ssh:// or rdp://">
          <Input className="font-mono" maxLength={500} value={f.url} onChange={(e) => setF({ ...f, url: e.target.value })} placeholder="ssh://db-1.internal" />
        </Field>
        <Field label="Tags" hint="Press Enter or comma to add.">
          <div className="flex flex-wrap items-center gap-1.5 rounded-md border border-line bg-panel px-2 py-1.5">
            {f.tags.map((t) => (
              <span key={t} className="inline-flex items-center gap-1 rounded bg-panel-2 px-1.5 py-0.5 text-xs">
                {t}
                <button type="button" onClick={() => setF({ ...f, tags: f.tags.filter((x) => x !== t) })} aria-label={`Remove ${t}`}>
                  <X className="size-3" />
                </button>
              </span>
            ))}
            <input
              data-bare className="min-w-[120px] flex-1 bg-transparent py-0.5 outline-none"
              value={tagInput}
              onChange={(e) => setTagInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === ",") {
                  e.preventDefault();
                  addTag();
                }
              }}
              onBlur={addTag}
              placeholder="prod, database"
            />
          </div>
        </Field>
        <Field label="Linked to" hint="The nodes, services or clusters this account belongs to.">
          <div className="space-y-2">
            <div className="flex flex-wrap gap-1.5">
              {f.links.map((l) => (
                <span key={`${l.kind}:${l.id}`} className="inline-flex items-center gap-1 rounded bg-accent-soft px-1.5 py-0.5 text-xs text-accent">
                  <Link2 className="size-3" /> {KIND_LABEL[l.kind as keyof typeof KIND_LABEL]}: {nameOf(l.kind, l.id)}
                  <button type="button" onClick={() => setF({ ...f, links: f.links.filter((x) => !(x.kind === l.kind && x.id === l.id)) })} aria-label="Remove link">
                    <X className="size-3" />
                  </button>
                </span>
              ))}
              {f.links.length === 0 && <span className="text-xs text-faint">Not linked</span>}
            </div>
            <Select
              value={pick}
              onChange={(e) => {
                const [k, id] = e.target.value.split(":");
                if (k && id && !f.links.some((l) => l.kind === k && l.id === id)) setF({ ...f, links: [...f.links, { kind: k as "node", id }] });
                setPick("");
              }}
            >
              <option value="">Add a link…</option>
              {(["cluster", "node", "service"] as const).map((k) => (
                <optgroup key={k} label={`${KIND_LABEL[k]}s`}>
                  {targets
                    .filter((t) => t.kind === k)
                    .map((t) => (
                      <option key={t.id} value={`${t.kind}:${t.id}`}>
                        {t.name}
                      </option>
                    ))}
                </optgroup>
              ))}
            </Select>
          </div>
        </Field>
        <Field label="Notes" hint="Not encrypted: do not put secrets here.">
          <TextArea rows={3} maxLength={4000} className="font-sans text-[13px]" value={f.notes} onChange={(e) => setF({ ...f, notes: e.target.value })} />
        </Field>
        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={busy}>
            {item ? (f.password ? <><RefreshCw className="size-4" /> Save and rotate</> : "Save") : "Save credential"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}
