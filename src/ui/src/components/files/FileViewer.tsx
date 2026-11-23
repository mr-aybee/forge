import { Check, Download, File as FileIcon, History, Loader2, Pencil, Save, WrapText, X } from "lucide-react";
import { Suspense, lazy, useEffect, useState } from "react";

import { ApiError } from "../../api/client";
import {
  EDIT_LIMIT,
  type FileRoot,
  READ_CHUNK,
  type Revision,
  type TextKind,
  VIEW_LIMIT,
  baseName,
  decodeB64,
  decodeText,
  detectText,
  downloadUrl,
  encodeText,
  lineEnding,
  listRevisions,
  readFile,
  readRevision,
  saveFile,
} from "../../api/files";
import { errorText, useToast } from "../../app/toast";
import { Badge, Button, ErrorNote, IconButton, Select, Skeleton, bytes, time } from "../ui";
import { LANGUAGES, type Language, detectLanguage } from "./lang";
import { HexView, Markdown, prettyJson } from "./Preview";

const CodeEditor = lazy(() => import("./Editor").then((m) => ({ default: m.CodeEditor })));
const MergeEditor = lazy(() => import("./Editor").then((m) => ({ default: m.MergeEditor })));

type Text = Extract<TextKind, { kind: "text" }>;

interface Loaded {
  bytes: Uint8Array;
  size: number;
  version?: string;
  eof: boolean;
  kind: TextKind;
  modified: number;
  /** Not on the node yet (new file). */
  isNew?: boolean;
}

const IMAGE = /\.(png|jpe?g|gif|webp|bmp|ico|svg)$/i;
const MIME: Record<string, string> = { svg: "image/svg+xml", jpg: "image/jpeg", jpeg: "image/jpeg", ico: "image/x-icon" };

/** Reads up to `limit` bytes of a file in chunks pinned to one version. */
async function readUpTo(serverId: string, root: string, path: string, limit: number) {
  const first = await readFile(serverId, root, path, 0);
  const head = decodeB64(first.content_b64 ?? "");
  const parts = [head];
  let got = head.length;
  let eof = first.eof;
  while (!eof && got < limit) {
    const r = await readFile(serverId, root, path, got, first.version);
    const b = decodeB64(r.content_b64 ?? "");
    if (!b.length) break;
    parts.push(b);
    got += b.length;
    eof = r.eof;
  }
  const all = new Uint8Array(got);
  let o = 0;
  for (const p of parts) {
    all.set(p, o);
    o += p.length;
  }
  return { bytes: all, size: first.size, version: first.version, eof, modified: first.modified_ms };
}

/** Viewer and editor for one file. */
export function FileViewer({ serverId, root, path, onClose, onSaved }: { serverId: string; root: FileRoot; path: string; onClose: () => void; onSaved: () => void }) {
  const [data, setData] = useState<Loaded | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [text, setText] = useState("");
  const [original, setOriginal] = useState("");
  const [lang, setLang] = useState<Language>("plain");
  const [wrap, setWrap] = useState(false);
  const [docKey, setDocKey] = useState(0);
  const [state, setState] = useState<"clean" | "saving" | "saved" | "validated">("clean");
  const [conflict, setConflict] = useState<{ current: string; version: string } | null>(null);
  const [revisions, setRevisions] = useState<Revision[] | null>(null);
  const [busy, setBusy] = useState(false);
  // Files open as a preview; Edit switches to the editor.
  const [editing, setEditing] = useState(false);
  const toast = useToast();
  const name = baseName(path);
  const canWrite = !root.read_only && root.permissions.write;

  const load = async () => {
    setError(null);
    setData(null);
    setConflict(null);
    try {
      const r = await readFile(serverId, root.id, path, 0);
      const head = decodeB64(r.content_b64 ?? "");
      const kind = detectText(head);
      const editable = kind.kind === "text" && r.size <= EDIT_LIMIT;
      const image = kind.kind === "binary" && IMAGE.test(name) && r.size <= 5 * 1024 * 1024;
      const full = editable || image ? await readUpTo(serverId, root.id, path, EDIT_LIMIT * 3) : { bytes: head, size: r.size, version: r.version, eof: r.eof, modified: r.modified_ms };
      const kind2 = IMAGE.test(name) && name.toLowerCase().endsWith(".svg") ? kind : detectText(full.bytes.subarray(0, 8192));
      setData({ ...full, kind: kind2 });
      if (kind2.kind === "text") {
        const t = decodeText(full.bytes, kind2);
        setText(t);
        setOriginal(t);
        setLang(detectLanguage(name, t.slice(0, 200).split("\n")[0] ?? ""));
      }
    } catch (e) {
      if (e instanceof ApiError && e.status === 404 && canWrite) {
        setEditing(true);
        // A new file: created on first save.
        setData({ bytes: new Uint8Array(), size: 0, eof: true, kind: { kind: "text", encoding: "utf-8", bom: false }, modified: 0, isNew: true });
        setText("");
        setOriginal("");
        setLang(detectLanguage(name, ""));
      } else {
        setError(errorText(e));
      }
    }
    setDocKey((k) => k + 1);
    setState("clean");
  };

  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [serverId, root.id, path]);

  const dirty = text !== original;
  const editable = !!data && data.kind.kind === "text" && data.size <= EDIT_LIMIT && data.eof;

  const save = async (content = text, version = data?.version) => {
    if (!data || data.kind.kind !== "text" || !canWrite) return;
    const kind: Text = data.kind;
    let validated = false;
    if (lang === "json" && content.trim()) {
      try {
        JSON.parse(content);
        validated = true;
      } catch (e) {
        toast(`Not saved: the JSON does not parse (${(e as Error).message})`, "error");
        return;
      }
    }
    setState("saving");
    try {
      // Keep the file's encoding and line endings.
      const eol = lineEnding(original);
      const out = encodeText(eol === "\r\n" ? content.replace(/\r?\n/g, "\r\n") : content, kind);
      const r = await saveFile(serverId, root.id, path, out, data.isNew ? undefined : version);
      setData({ ...data, version: r.version, size: r.size, isNew: false, bytes: out });
      setOriginal(content);
      setText(content);
      setConflict(null);
      setState(validated ? "validated" : "saved");
      onSaved();
    } catch (e) {
      setState("clean");
      if (e instanceof ApiError && e.status === 409) {
        // Someone changed the file: compare with what is there now.
        try {
          const cur = await readUpTo(serverId, root.id, path, EDIT_LIMIT * 3);
          const ck = detectText(cur.bytes.subarray(0, 8192));
          setConflict({ current: ck.kind === "text" ? decodeText(cur.bytes, ck) : "", version: cur.version });
        } catch (e2) {
          toast(errorText(e2), "error");
        }
      } else {
        toast(errorText(e), "error");
      }
    }
  };

  const showRevisions = async () => {
    setBusy(true);
    try {
      setRevisions(await listRevisions(serverId, root.id, path));
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };

  const loadRevision = async (rev: Revision) => {
    if (!data || data.kind.kind !== "text") return;
    try {
      const r = await readRevision(serverId, root.id, path, rev.id);
      setText(decodeText(decodeB64(r.content_b64 ?? ""), data.kind));
      setDocKey((k) => k + 1);
      setRevisions(null);
      toast(`Revision from ${time(rev.saved_at)} loaded; save to restore it`, "ok");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  let body: React.ReactNode;
  if (error) {
    body = (
      <div className="p-3">
        <ErrorNote error={new Error(error)} />
      </div>
    );
  } else if (!data) {
    body = (
      <div className="p-3">
        <Skeleton rows={10} />
      </div>
    );
  } else if (data.kind.kind === "binary") {
    const ext = name.split(".").pop()?.toLowerCase() ?? "";
    body =
      IMAGE.test(name) && data.eof ? (
        <div className="grid place-items-center p-4">
          {/* Images render inert as data: URLs (no scripts, even for SVG). */}
          <img className="max-h-[65vh] max-w-full" alt={name} src={`data:${MIME[ext] ?? `image/${ext}`};base64,${btoa(Array.from(data.bytes, (b) => String.fromCharCode(b)).join(""))}`} />
        </div>
      ) : (
        <HexView bytes={data.bytes.subarray(0, 4096)} total={data.size} />
      );
  } else if (conflict) {
    body = (
      <div className="flex min-h-0 flex-1 flex-col">
        <div className="border-b border-line bg-warn-soft px-3 py-2 text-xs text-warn">
          The file changed on the node since you opened it. Left: the file now. Right: your edit. Merge on the right, then save.
        </div>
        <div className="h-[60vh] min-h-0">
          <Suspense fallback={<Skeleton rows={10} />}>
            <MergeEditor current={conflict.current} mine={text} lang={lang} onChange={setText} />
          </Suspense>
        </div>
        <div className="flex justify-end gap-2 border-t border-line px-3 py-2">
          <Button onClick={() => void load()}>Discard mine and reload</Button>
          <Button variant="primary" onClick={() => void save(text, conflict.version)}>
            Save merged version
          </Button>
        </div>
      </div>
    );
  } else if (editable && !editing) {
    const pretty = lang === "json" ? prettyJson(text) : null;
    body =
      lang === "markdown" ? (
        <div className="max-h-[65vh] overflow-auto">
          <Markdown text={text} />
        </div>
      ) : (
        <div className="h-[65vh] min-h-0">
          <Suspense fallback={<Skeleton rows={10} />}>
            <CodeEditor value={pretty ?? text} docKey={`${path}:${docKey}:preview`} lang={lang} readOnly wrap={wrap} onChange={() => undefined} />
          </Suspense>
        </div>
      );
  } else if (editable) {
    body = (
      <div className="h-[65vh] min-h-0">
        <Suspense fallback={<Skeleton rows={10} />}>
          <CodeEditor value={text} docKey={`${path}:${docKey}`} lang={lang} readOnly={!canWrite} wrap={wrap} onChange={setText} onSave={() => dirty && void save()} />
        </Suspense>
      </div>
    );
  } else {
    body = <PagedView serverId={serverId} root={root.id} path={path} data={data} />;
  }

  return (
    <aside className="flex min-w-0 flex-col border-t border-line lg:border-l lg:border-t-0" aria-label={`Viewer: ${name}`}>
      <div className="flex flex-wrap items-center gap-2 border-b border-line px-3 py-2">
        <FileIcon className="size-4 shrink-0 text-faint" />
        <span className="min-w-0 flex-1 truncate font-mono text-xs" title={path}>
          {path}
          {dirty && " •"}
        </span>
        {data && (
          <span className="text-[11px] text-faint">
            {data.isNew ? "new file" : bytes(data.size)}
            {data.kind.kind === "text" ? ` · ${data.kind.encoding}${lineEnding(original) === "\r\n" ? " · CRLF" : ""}` : " · binary"}
          </span>
        )}
        {state === "saved" && <Badge tone="ok">Saved</Badge>}
        {state === "validated" && (
          <Badge tone="ok">
            <Check className="size-3" /> Saved · Validated
          </Badge>
        )}
        {editable && (
          <>
            <Select aria-label="Language" className="!w-32 !text-xs" value={lang} onChange={(e) => setLang(e.target.value as Language)}>
              {LANGUAGES.map((l) => (
                <option key={l} value={l}>
                  {l}
                </option>
              ))}
            </Select>
            <IconButton label={wrap ? "No line wrap" : "Wrap lines"} onClick={() => setWrap((w) => !w)}>
              <WrapText className={`size-4 ${wrap ? "text-accent" : ""}`} />
            </IconButton>
            {!editing && canWrite && (
              <Button onClick={() => setEditing(true)} title="Open in the editor">
                <Pencil className="size-4" /> Edit
              </Button>
            )}
            {root.kind !== "container" && !data?.isNew && (
              <IconButton label="Revisions" onClick={() => void showRevisions()} disabled={busy}>
                <History className="size-4" />
              </IconButton>
            )}
            {canWrite && editing && (
              <Button variant="primary" disabled={!dirty || state === "saving" || !!conflict} onClick={() => void save()} title="Save (Ctrl+S)">
                {state === "saving" ? <Loader2 className="size-4 animate-spin" /> : <Save className="size-4" />} Save
              </Button>
            )}
          </>
        )}
        {!data?.isNew && (
          <a className="inline-flex rounded p-1 hover:bg-panel-2 hover:text-accent" href={downloadUrl(serverId, root.id, path)} download={name} aria-label="Download" title="Download">
            <Download className="size-4" />
          </a>
        )}
        <IconButton label="Close viewer" onClick={() => (!dirty || window.confirm("Discard unsaved changes?")) && onClose()}>
          <X className="size-4" />
        </IconButton>
      </div>
      {revisions && (
        <div className="border-b border-line px-3 py-2 text-xs">
          <div className="mb-1 flex items-center justify-between">
            <span className="font-medium">Saved revisions</span>
            <IconButton label="Close revisions" onClick={() => setRevisions(null)}>
              <X className="size-3.5" />
            </IconButton>
          </div>
          {revisions.length === 0 ? (
            <p className="text-faint">No earlier versions. Forge keeps the last 10 versions saved from the console.</p>
          ) : (
            <ul className="max-h-40 divide-y divide-line overflow-y-auto">
              {revisions.map((r) => (
                <li key={r.id} className="flex items-center gap-3 py-1">
                  <span className="flex-1">{time(r.saved_at)}</span>
                  <span className="font-mono text-faint">{bytes(r.size)}</span>
                  <Button variant="ghost" onClick={() => void loadRevision(r)}>
                    Load
                  </Button>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
      {body}
    </aside>
  );
}

/** Read-only pages of a large text file. */
function PagedView({ serverId, root, path, data }: { serverId: string; root: string; path: string; data: Loaded }) {
  const [chunks, setChunks] = useState<Uint8Array[]>([data.bytes]);
  const [eof, setEof] = useState(data.eof);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const loaded = chunks.reduce((n, c) => n + c.length, 0);
  const kind = data.kind.kind === "text" ? data.kind : null;
  const more = async () => {
    setBusy(true);
    try {
      const r = await readFile(serverId, root, path, loaded, data.version);
      setChunks((c) => [...c, decodeB64(r.content_b64 ?? "")]);
      setEof(r.eof);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  const all = new Uint8Array(loaded);
  let o = 0;
  for (const c of chunks) {
    all.set(c, o);
    o += c.length;
  }
  return (
    <>
      <div className="border-b border-line px-3 py-1.5 text-[11px] text-faint">
        Larger than 2 MB: shown read-only{data.size > VIEW_LIMIT ? `; files over ${bytes(VIEW_LIMIT)} show their start only` : ""}.
      </div>
      <pre className="max-h-[65vh] min-h-48 overflow-auto p-3 font-mono text-xs leading-5 whitespace-pre">{kind ? decodeText(all, { ...kind, bom: kind.bom && loaded >= 3 }) : ""}</pre>
      {error && <p className="px-3 text-xs text-bad">{error}</p>}
      {!eof && (
        <div className="flex items-center gap-3 border-t border-line px-3 py-2 text-xs text-faint">
          <span>
            Showing {bytes(loaded)} of {bytes(data.size)}.
          </span>
          {data.size <= VIEW_LIMIT && loaded < VIEW_LIMIT ? (
            <Button variant="ghost" onClick={() => void more()} disabled={busy}>
              {busy && <Loader2 className="size-4 animate-spin" />} Load more ({bytes(READ_CHUNK)})
            </Button>
          ) : (
            <span>Download the file to see the rest.</span>
          )}
        </div>
      )}
    </>
  );
}
