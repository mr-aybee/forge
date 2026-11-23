import { useQuery } from "@tanstack/react-query";
import { Loader2, RotateCcw, Search, Trash2 } from "lucide-react";
import { useState } from "react";

import {
  type FileRoot,
  type SearchResult,
  addRoot,
  deleteRoot,
  uniqueRoots,
  listTrash,
  purgeTrash,
  restoreTrash,
  searchFiles,
} from "../../api/files";
import { errorText, useToast } from "../../app/toast";
import { Badge, Button, Confirm, Drawer, Empty, ErrorNote, Field, Input, Modal, Select, Skeleton, bytes, time } from "../ui";

/** Trash entries of one root: restore or delete for good. */
export function TrashDrawer({ serverId, root, onClose, onChanged }: { serverId: string; root: FileRoot; onClose: () => void; onChanged: () => void }) {
  const trash = useQuery({ queryKey: ["file-trash", serverId, root.id], queryFn: () => listTrash(serverId, root.id), retry: false });
  const [purge, setPurge] = useState<string | "all" | null>(null);
  const toast = useToast();
  const canDelete = !root.read_only && root.permissions.delete;
  const act = async (f: () => Promise<unknown>, msg: string) => {
    try {
      await f();
      toast(msg, "ok");
      void trash.refetch();
      onChanged();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <Drawer open onOpenChange={(o) => !o && onClose()} title={`Trash · ${root.label}`} wide>
      <p className="mb-3 text-xs text-muted">Deleted files stay here for 30 days, up to 10 GB per node; the oldest go first.</p>
      {trash.isLoading ? (
        <Skeleton rows={5} />
      ) : trash.error ? (
        <ErrorNote error={trash.error} />
      ) : !trash.data?.length ? (
        <Empty icon={<Trash2 className="size-5" />} title="The trash is empty" />
      ) : (
        <>
          <ul className="divide-y divide-line rounded-lg border border-line text-xs">
            {trash.data.map((e) => (
              <li key={e.id} className="flex items-center gap-3 px-3 py-2">
                <span className="min-w-0 flex-1 truncate font-mono" title={e.path}>
                  {e.path}
                </span>
                <span className="text-faint">{time(e.deleted_at)}</span>
                <span className="w-20 text-right font-mono text-faint">{bytes(e.size)}</span>
                {canDelete && root.permissions.write && (
                  <Button variant="ghost" onClick={() => void act(() => restoreTrash(serverId, root.id, e.id), "Restored")}>
                    <RotateCcw className="size-4" /> Restore
                  </Button>
                )}
                {canDelete && (
                  <Button variant="ghost" onClick={() => setPurge(e.id)}>
                    <Trash2 className="size-4" />
                  </Button>
                )}
              </li>
            ))}
          </ul>
          {canDelete && (
            <div className="mt-3 flex justify-end">
              <Button variant="danger" onClick={() => setPurge("all")}>
                Empty trash
              </Button>
            </div>
          )}
        </>
      )}
      <Confirm
        open={purge !== null}
        onOpenChange={(o) => !o && setPurge(null)}
        title={purge === "all" ? "Empty the trash?" : "Delete for good?"}
        body={purge === "all" ? `Everything deleted from ${root.label} is removed for good.` : "This entry is removed for good."}
        confirmLabel="Delete"
        onConfirm={() => {
          const id = purge === "all" ? undefined : (purge ?? undefined);
          setPurge(null);
          void act(() => purgeTrash(serverId, root.id, id), "Deleted for good");
        }}
      />
    </Drawer>
  );
}

/** Filename and content search below the current folder. */
export function SearchDrawer({ serverId, root, start, onClose, onOpen }: { serverId: string; root: FileRoot; start: string; onClose: () => void; onOpen: (path: string) => void }) {
  const [name, setName] = useState("");
  const [content, setContent] = useState("");
  const [regex, setRegex] = useState(false);
  const [caseSensitive, setCase] = useState(false);
  const [word, setWord] = useState(false);
  const [exclude, setExclude] = useState("node_modules/**, .git/**");
  const [result, setResult] = useState<SearchResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = async (after?: string) => {
    setBusy(true);
    setError(null);
    try {
      const r = await searchFiles(serverId, root.id, start, {
        name: name.trim(),
        content,
        regex,
        case_sensitive: caseSensitive,
        whole_word: word,
        context: 1,
        exclude: exclude
          .split(",")
          .map((s) => s.trim())
          .filter(Boolean),
        after,
      });
      setResult((prev) => (after && prev ? { ...r, files: [...prev.files, ...r.files] } : r));
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const status = { complete: "complete", truncated: "stopped at a limit", timeout: "stopped after 50 s" } as const;
  const full = (rel: string) => (start === "/" ? `/${rel}` : `${start}/${rel}`);

  return (
    <Drawer open onOpenChange={(o) => !o && onClose()} title={`Search in ${start}`} wide>
      <form
        className="grid gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          void run();
        }}
      >
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="File name" hint="Glob, such as *.conf">
            <Input className="font-mono" value={name} onChange={(e) => setName(e.target.value)} placeholder="*.conf" />
          </Field>
          <Field label="Containing">
            <Input className="font-mono" value={content} onChange={(e) => setContent(e.target.value)} placeholder="server_name" />
          </Field>
        </div>
        <Field label="Exclude" hint="Comma-separated globs">
          <Input className="font-mono" value={exclude} onChange={(e) => setExclude(e.target.value)} />
        </Field>
        <div className="flex flex-wrap gap-4 text-xs text-muted">
          <label className="flex items-center gap-1.5">
            <input type="checkbox" checked={regex} onChange={(e) => setRegex(e.target.checked)} /> Regular expression
          </label>
          <label className="flex items-center gap-1.5">
            <input type="checkbox" checked={caseSensitive} onChange={(e) => setCase(e.target.checked)} /> Match case
          </label>
          <label className="flex items-center gap-1.5">
            <input type="checkbox" checked={word} onChange={(e) => setWord(e.target.checked)} /> Whole word
          </label>
        </div>
        <div className="flex justify-end">
          <Button variant="primary" type="submit" disabled={busy || (!name.trim() && !content)}>
            {busy ? <Loader2 className="size-4 animate-spin" /> : <Search className="size-4" />} Search
          </Button>
        </div>
      </form>
      {error && <p className="mt-3 text-xs text-bad">{error}</p>}
      {result && (
        <div className="mt-4">
          <div className="mb-2 flex items-center gap-2 text-xs text-muted">
            <Badge tone={result.status === "complete" ? "ok" : "warn"}>{status[result.status]}</Badge>
            {result.files.length} files · {result.scanned_files} scanned · {bytes(result.scanned_bytes)} read
            {result.next && (
              <Button variant="ghost" disabled={busy} onClick={() => void run(result.next ?? undefined)}>
                Continue
              </Button>
            )}
          </div>
          {result.files.length === 0 ? (
            <Empty title="No matches" />
          ) : (
            <ul className="divide-y divide-line rounded-lg border border-line text-xs">
              {result.files.map((f) => (
                <li key={f.path} className="px-3 py-2">
                  <button className="font-mono text-accent hover:underline" onClick={() => onOpen(full(f.path))}>
                    {f.path}
                  </button>
                  {f.matches?.map((m) => (
                    <div key={m.line} className="mt-1 font-mono">
                      <span className="mr-2 inline-block w-10 text-right text-faint">{m.line}</span>
                      <span className="whitespace-pre-wrap">{m.text}</span>
                    </div>
                  ))}
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </Drawer>
  );
}

/** Approve service and container directories as roots (file.admin, MFA). */
export function RootsDialog({ serverId, roots, os, onClose, onChanged }: { serverId: string; roots: FileRoot[]; os: "linux" | "windows"; onClose: () => void; onChanged: () => void }) {
  const [kind, setKind] = useState<"service" | "container">("service");
  const [label, setLabel] = useState("");
  const [path, setPath] = useState("");
  const [container, setContainer] = useState("");
  const [readOnly, setReadOnly] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const toast = useToast();
  const approved = uniqueRoots(roots).filter((r) => r.kind !== "node");

  const add = async () => {
    setBusy(true);
    setError(null);
    try {
      await addRoot(serverId, { kind, label, path, container: kind === "container" ? container.trim() : undefined, read_only: readOnly });
      toast("Root approved", "ok");
      setLabel("");
      setPath("");
      onChanged();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const remove = async (id: string) => {
    try {
      await deleteRoot(serverId, id);
      onChanged();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title="File roots" description="Operators see only the directories approved here; the whole node needs an administrator role." wide>
      {approved.length > 0 && (
        <ul className="mb-4 divide-y divide-line rounded-lg border border-line text-xs">
          {approved.map((r) => (
            <li key={r.id} className="flex items-center gap-3 px-3 py-2">
              <span className="font-medium">{r.label}</span>
              <span className="min-w-0 flex-1 truncate font-mono text-faint">
                {r.container ? `${r.container.slice(0, 12)}:` : ""}
                {r.base}
              </span>
              <Badge tone={r.read_only ? "muted" : "warn"}>{r.read_only ? "read-only" : "read-write"}</Badge>
              <Button variant="ghost" onClick={() => void remove(r.id)}>
                <Trash2 className="size-4" />
              </Button>
            </li>
          ))}
        </ul>
      )}
      <form
        className="grid gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          void add();
        }}
      >
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="Kind">
            <Select value={kind} onChange={(e) => setKind(e.target.value as "service" | "container")}>
              <option value="service">Service directory</option>
              {os === "linux" && <option value="container">Container directory</option>}
            </Select>
          </Field>
          <Field label="Label">
            <Input value={label} onChange={(e) => setLabel(e.target.value)} placeholder="web app" />
          </Field>
        </div>
        {kind === "container" && (
          <Field label="Container id" hint="The full 64-character id (docker inspect)">
            <Input className="font-mono" value={container} onChange={(e) => setContainer(e.target.value)} />
          </Field>
        )}
        <Field label={kind === "container" ? "Path inside the container" : "Directory"}>
          <Input className="font-mono" value={path} onChange={(e) => setPath(e.target.value)} placeholder={os === "windows" ? "/D:/sites/app" : "/srv/app"} />
        </Field>
        <label className="flex items-center gap-2 text-xs text-muted">
          <input type="checkbox" checked={readOnly} onChange={(e) => setReadOnly(e.target.checked)} /> Read-only
        </label>
        {error && <p className="text-xs text-bad">{error}</p>}
        <div className="flex justify-end gap-2">
          <Button type="button" onClick={onClose}>
            Close
          </Button>
          <Button variant="primary" type="submit" disabled={busy || !label.trim() || !path.trim()}>
            {busy && <Loader2 className="size-4 animate-spin" />} Approve root
          </Button>
        </div>
      </form>
    </Modal>
  );
}
