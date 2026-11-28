import { Loader2 } from "lucide-react";
import { useEffect, useState } from "react";

import {
  type ArchiveResult,
  type BulkResult,
  type FileEntry,
  type FileRoot,
  type PermResult,
  baseName,
  bulk,
  chmod,
  chown,
  compress,
  extract,
  getAcl,
  joinPath,
  parentPath,
} from "../../api/files";
import { errorText } from "../../app/toast";
import { Badge, Button, Field, Input, Modal } from "../ui";

const BITS = [
  ["Owner", 6],
  ["Group", 3],
  ["Others", 0],
] as const;

/** Permissions (rwx grid + octal), owner and group; ACL view on Windows. */
export function PermissionsDialog({
  serverId,
  root,
  dir,
  entries,
  windows,
  onClose,
  onDone,
}: {
  serverId: string;
  root: FileRoot;
  dir: string;
  entries: FileEntry[];
  windows: boolean;
  onClose: () => void;
  onDone: (msg: string) => void;
}) {
  const first = entries[0];
  const [mode, setMode] = useState(((first?.mode ?? 0o644) & 0o7777).toString(8).padStart(4, "0"));
  const [owner, setOwner] = useState(entries.length === 1 ? (first?.owner ?? "") : "");
  const [group, setGroup] = useState(entries.length === 1 ? (first?.group ?? "") : "");
  const [recursive, setRecursive] = useState(false);
  const [preview, setPreview] = useState<PermResult[] | null>(null);
  const [acl, setAcl] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const paths = entries.map((e) => joinPath(dir, e.name));
  const value = Number.parseInt(mode, 8);
  const validMode = /^[0-7]{3,4}$/.test(mode);
  const special = validMode && value > 0o777;
  const changeOwner = entries.length > 1 ? owner !== "" || group !== "" : owner !== (first?.owner ?? "") || group !== (first?.group ?? "");

  useEffect(() => {
    if (windows && first) {
      void getAcl(serverId, root.id, joinPath(dir, first.name))
        .then((r) => setAcl(r.acl))
        .catch((e) => setAcl(errorText(e)));
    }
  }, [windows, first, serverId, root.id, dir]);

  const toggle = (shift: number, bit: number) => {
    const v = (validMode ? value : 0) ^ (bit << shift);
    setMode((v & 0o7777).toString(8).padStart(4, "0"));
  };

  const run = async (dryRun: boolean) => {
    setBusy(true);
    setError(null);
    try {
      const out: PermResult[] = [];
      for (const p of paths) {
        out.push(await chmod(serverId, root.id, p, mode, recursive, dryRun));
        if (changeOwner) out.push(await chown(serverId, root.id, p, owner, group, recursive, dryRun));
      }
      if (dryRun) {
        setPreview(out);
      } else {
        onDone(`Changed ${out.reduce((n, r) => n + r.changed, 0)} entries`);
      }
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  if (windows) {
    return (
      <Modal open onOpenChange={(o) => !o && onClose()} title="Permissions" description={paths[0]} wide>
        <p className="mb-2 text-xs text-muted">Windows access control lists are shown read-only. Change them on the node for now.</p>
        <pre className="max-h-[50vh] overflow-auto rounded-md bg-panel-2 p-3 font-mono text-xs whitespace-pre-wrap">{acl ?? "Loading…"}</pre>
      </Modal>
    );
  }

  const changes = preview?.reduce((n, r) => n + r.changed, 0) ?? 0;
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title="Permissions" description={entries.length === 1 ? paths[0] : `${entries.length} items`} wide>
      <div className="grid gap-4">
        <table className="w-fit text-xs">
          <thead>
            <tr className="text-faint">
              <th className="pr-4 text-left font-normal" />
              {["Read", "Write", "Run"].map((h) => (
                <th key={h} className="px-3 font-normal">
                  {h}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {BITS.map(([label, shift]) => (
              <tr key={label}>
                <td className="pr-4">{label}</td>
                {[4, 2, 1].map((bit) => (
                  <td key={bit} className="px-3 text-center">
                    <input type="checkbox" aria-label={`${label} ${bit}`} checked={validMode && ((value >> shift) & bit) !== 0} onChange={() => toggle(shift, bit)} />
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
        <div className="grid gap-3 sm:grid-cols-3">
          <Field label="Mode (octal)">
            <Input className="font-mono" value={mode} onChange={(e) => setMode(e.target.value.trim())} />
          </Field>
          <Field label="Owner">
            <Input className="font-mono" value={owner} onChange={(e) => setOwner(e.target.value.trim())} placeholder="unchanged" />
          </Field>
          <Field label="Group">
            <Input className="font-mono" value={group} onChange={(e) => setGroup(e.target.value.trim())} placeholder="unchanged" />
          </Field>
        </div>
        {special && <p className="text-xs text-warn">setuid, setgid or sticky bits are set; only administrators can apply them.</p>}
        <label className="flex items-center gap-2 text-xs text-muted">
          <input type="checkbox" checked={recursive} onChange={(e) => (setRecursive(e.target.checked), setPreview(null))} /> Apply to everything inside folders too
        </label>
        {preview && (
          <div className="rounded-md border border-line p-2 text-xs">
            <p className="mb-1">
              {changes} {changes === 1 ? "entry changes" : "entries change"}
              {preview.some((r) => r.skipped) ? `, ${preview.reduce((n, r) => n + r.skipped, 0)} skipped (links, special or protected files)` : ""}.
            </p>
            <ul className="max-h-40 overflow-y-auto font-mono">
              {preview.flatMap((r) => r.sample).slice(0, 30).map((s, i) => (
                <li key={i}>
                  {s.path}: {s.before} → {s.after}
                </li>
              ))}
            </ul>
          </div>
        )}
        {error && <p className="text-xs text-bad">{error}</p>}
        <div className="flex justify-end gap-2">
          <Button onClick={onClose}>Cancel</Button>
          <Button disabled={busy || !validMode} onClick={() => void run(true)}>
            Preview
          </Button>
          <Button variant="primary" disabled={busy || !validMode || !preview} onClick={() => void run(false)} title={preview ? "" : "Preview first"}>
            {busy && <Loader2 className="size-4 animate-spin" />} Apply
          </Button>
        </div>
      </div>
    </Modal>
  );
}

const FORMATS = {
  "tar.gz": { label: ".tar.gz", note: "Smaller; keeps Linux permissions and owners." },
  zip: { label: ".zip", note: "Opens anywhere, including Windows and macOS." },
} as const;
type ArchiveFormat = keyof typeof FORMATS;

/** "1 item" / "3 items". */
export const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** Why an archive name can't be used, or null when it can. */
function nameProblem(base: string, file: string, existing: readonly string[] | undefined): string | null {
  const b = base.trim();
  if (!b) return "Enter a name.";
  if (/[\\/]/.test(b)) return "The name can't contain / or \\.";
  // eslint-disable-next-line no-control-regex
  if (/[<>:"|?*\u0000-\u001f]/.test(b)) return 'The name can\'t contain < > : " | ? * or control characters.';
  if (b === "." || b === "..") return "Choose a different name.";
  if (existing?.includes(file)) return `${file} already exists here. Choose another name.`;
  return null;
}

/** Compress the selection into a new .tar.gz or .zip in the current folder. */
export function CompressDialog({
  serverId,
  root,
  dir,
  entries,
  windows = false,
  existing,
  onClose,
  onDone,
}: {
  serverId: string;
  root: FileRoot;
  dir: string;
  entries: FileEntry[];
  windows?: boolean;
  /** Names already in `dir`, to flag a conflict before the request. */
  existing?: readonly string[];
  onClose: () => void;
  onDone: (msg: string) => void;
}) {
  // Linux nodes: .tar.gz (default) or .zip. Windows nodes: .zip only.
  const formats: ArchiveFormat[] = windows ? ["zip"] : ["tar.gz", "zip"];
  const [format, setFormat] = useState<ArchiveFormat>(formats[0] ?? "zip");
  const first = entries[0];
  // A single file "notes.txt" suggests "notes"; folders keep their name.
  const suggested = entries.length === 1 && first ? (first.kind === "file" ? first.name.replace(/\.[^.]+$/, "") || first.name : first.name) : "archive";
  const [base, setBase] = useState(suggested);
  const [touched, setTouched] = useState(false);
  const name = `${base.trim()}.${format}`;
  const target = joinPath(dir, name);
  const problem = nameProblem(base, name, existing);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<ArchiveResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const run = async () => {
    setTouched(true);
    if (problem || busy) return;
    setBusy(true);
    setError(null);
    try {
      const r = await compress(serverId, root.id, entries.map((e) => joinPath(dir, e.name)), target);
      if (r.skipped.length) setResult(r);
      else onDone(`Created ${name} (${plural(r.entries, "entry", "entries")})`);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  const shown = entries.slice(0, 3).map((e) => e.name);
  const what = entries.length === 1 ? (first?.name ?? "") : `${shown.join(", ")}${entries.length > 3 ? ` and ${entries.length - 3} more` : ""}`;
  const showProblem = !!problem && (touched || base.trim() !== "");
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={`Compress ${plural(entries.length, "item")}`}>
      {result ? (
        <Skipped result={result} onClose={() => onDone(`Created ${name}`)} />
      ) : (
        <form
          className="grid gap-4 text-xs"
          onSubmit={(e) => {
            e.preventDefault();
            void run();
          }}
        >
          <p className="-mt-2 truncate font-mono text-[11px] text-muted" title={entries.map((e) => e.name).join("\n")}>
            {what}
          </p>

          <div className="grid gap-1.5">
            <label htmlFor="archive-name" className="font-medium">
              Archive name
            </label>
            <div className={`flex min-w-0 items-stretch overflow-hidden rounded-md border bg-panel focus-within:ring-2 focus-within:ring-accent/40 ${showProblem ? "border-bad" : "border-line-strong focus-within:border-accent"}`}>
              <input
                id="archive-name"
                autoFocus
                spellCheck={false}
                autoComplete="off"
                data-bare className="min-w-0 flex-1 bg-transparent px-2.5 py-1.5 font-mono text-[13px] outline-none"
                value={base}
                aria-invalid={showProblem}
                aria-describedby="archive-name-note"
                onChange={(e) => setBase(e.target.value)}
                onBlur={() => setTouched(true)}
              />
              <span className="flex shrink-0 items-center border-l border-line bg-panel-2 px-2.5 font-mono text-[13px] text-muted" aria-label={`extension .${format}`}>
                .{format}
              </span>
            </div>
            <p id="archive-name-note" className={`min-h-4 truncate ${showProblem ? "text-bad" : "text-faint"}`} title={showProblem ? (problem ?? "") : target}>
              {showProblem ? (
                problem
              ) : (
                <>
                  Creates <span className="font-mono text-muted">{target}</span>
                </>
              )}
            </p>
          </div>

          <fieldset className="grid gap-1.5">
            <legend className="mb-1.5 font-medium">Format</legend>
            <div className="grid gap-1.5 sm:grid-cols-2">
              {(Object.keys(FORMATS) as ArchiveFormat[]).map((f) => {
                const allowed = formats.includes(f);
                const on = format === f;
                return (
                  <label
                    key={f}
                    title={allowed ? undefined : "Windows nodes create .zip archives only"}
                    className={`flex items-start gap-2 rounded-md border px-2.5 py-2 transition focus-within:ring-2 focus-within:ring-accent/40 ${on ? "border-accent bg-accent-soft" : "border-line hover:border-line-strong"} ${allowed ? "cursor-pointer" : "cursor-not-allowed opacity-50"}`}
                  >
                    <input type="radio" name="archive-format" className="mt-0.5" value={f} checked={on} disabled={!allowed} onChange={() => setFormat(f)} />
                    <span className="min-w-0">
                      <span className="block font-mono text-[13px] font-medium">{FORMATS[f].label}</span>
                      <span className="block text-muted">{allowed ? FORMATS[f].note : "Not available on Windows nodes."}</span>
                    </span>
                  </label>
                );
              })}
            </div>
          </fieldset>

          {error && (
            <p role="alert" className="rounded-md bg-bad-soft px-2.5 py-1.5 text-bad">
              {error}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <Button type="button" onClick={onClose}>
              Cancel
            </Button>
            <Button variant="primary" type="submit" disabled={busy || !!problem}>
              {busy && <Loader2 className="size-4 animate-spin" />} Compress
            </Button>
          </div>
        </form>
      )}
    </Modal>
  );
}

/** Extract an archive into a new folder (default: named after it). */
export function ExtractDialog({ serverId, root, path, onClose, onDone }: { serverId: string; root: FileRoot; path: string; onClose: () => void; onDone: (msg: string) => void }) {
  const stem = baseName(path).replace(/\.(zip|tar\.gz|tgz|tar)$/i, "");
  const [dest, setDest] = useState(joinPath(parentPath(path), stem || "extracted"));
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<ArchiveResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const run = async () => {
    setBusy(true);
    setError(null);
    try {
      const r = await extract(serverId, root.id, path, dest.trim());
      if (r.skipped.length) setResult(r);
      else onDone(`Extracted ${r.entries} entries`);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title="Extract" description={path}>
      {result ? (
        <Skipped result={result} onClose={() => onDone(`Extracted ${result.entries} entries`)} />
      ) : (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void run();
          }}
        >
          <Field label="Into a new folder">
            <Input autoFocus className="font-mono" value={dest} onChange={(e) => setDest(e.target.value)} />
          </Field>
          <p className="mt-2 text-xs text-faint">Unsafe entries (absolute paths, `..`) stop the extraction; links and device files are skipped. Nothing is written until the archive checks out.</p>
          {error && <p className="mt-3 text-xs text-bad">{error}</p>}
          <div className="mt-5 flex justify-end gap-2">
            <Button type="button" onClick={onClose}>
              Cancel
            </Button>
            <Button variant="primary" type="submit" disabled={busy || !dest.trim()}>
              {busy && <Loader2 className="size-4 animate-spin" />} Extract
            </Button>
          </div>
        </form>
      )}
    </Modal>
  );
}

function Skipped({ result, onClose }: { result: ArchiveResult; onClose: () => void }) {
  return (
    <div className="text-xs">
      <p className="mb-2">
        Done: {result.entries} entries. {result.skipped.length} skipped:
      </p>
      <ul className="max-h-48 overflow-y-auto font-mono">
        {result.skipped.slice(0, 100).map((s, i) => (
          <li key={i}>
            {s.path ?? s.entry} <Badge tone="muted">{s.reason}</Badge>
          </li>
        ))}
      </ul>
      <div className="mt-4 flex justify-end">
        <Button variant="primary" onClick={onClose}>
          Close
        </Button>
      </div>
    </div>
  );
}

/** Copy or move many items into a folder, with a result per item. */
export function BulkDialog({
  serverId,
  root,
  dir,
  entries,
  action,
  onClose,
  onDone,
}: {
  serverId: string;
  root: FileRoot;
  dir: string;
  entries: FileEntry[];
  action: "copy" | "move";
  onClose: () => void;
  onDone: (msg: string) => void;
}) {
  const [to, setTo] = useState(dir);
  const [overwrite, setOverwrite] = useState(false);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<BulkResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const run = async () => {
    setBusy(true);
    setError(null);
    try {
      const r = await bulk(serverId, root.id, action, entries.map((e) => joinPath(dir, e.name)), to.trim(), overwrite);
      if (r.failed) setResult(r);
      else onDone(`${action === "copy" ? "Copied" : "Moved"} ${r.ok} item(s)`);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={action === "copy" ? "Copy to" : "Move to"} description={`${entries.length} item(s)`}>
      {result ? (
        <BulkResults result={result} onClose={() => onDone(`${result.ok} done, ${result.failed} failed`)} />
      ) : (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void run();
          }}
        >
          <Field label="Destination folder">
            <Input autoFocus className="font-mono" value={to} onChange={(e) => setTo(e.target.value)} />
          </Field>
          <label className="mt-3 flex items-center gap-2 text-xs text-muted">
            <input type="checkbox" checked={overwrite} onChange={(e) => setOverwrite(e.target.checked)} /> Replace items that exist there
          </label>
          {error && <p className="mt-3 text-xs text-bad">{error}</p>}
          <div className="mt-5 flex justify-end gap-2">
            <Button type="button" onClick={onClose}>
              Cancel
            </Button>
            <Button variant="primary" type="submit" disabled={busy || !to.trim() || to.trim() === dir}>
              {busy && <Loader2 className="size-4 animate-spin" />} {action === "copy" ? "Copy" : "Move"}
            </Button>
          </div>
        </form>
      )}
    </Modal>
  );
}

export function BulkResults({ result, onClose }: { result: BulkResult; onClose: () => void }) {
  return (
    <div className="text-xs">
      <p className="mb-2">
        {result.ok} done, {result.failed} failed.
      </p>
      <ul className="max-h-60 overflow-y-auto font-mono">
        {result.results
          .filter((r) => !r.ok)
          .map((r) => (
            <li key={r.path}>
              {r.path}: <span className="text-bad">{r.error}</span>
            </li>
          ))}
      </ul>
      <div className="mt-4 flex justify-end">
        <Button variant="primary" onClick={onClose}>
          Close
        </Button>
      </div>
    </div>
  );
}

