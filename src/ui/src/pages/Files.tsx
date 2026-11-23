import { useQueryClient } from "@tanstack/react-query";
import {
  ArrowUp,
  ChevronRight,
  Copy,
  CopyPlus,
  Download,
  FileArchive,
  Eye,
  EyeOff,
  File as FileIcon,
  FilePlus,
  Fingerprint,
  Folder,
  FolderInput,
  FolderPlus,
  FolderUp,
  HardDrive,
  KeyRound,
  Link2,
  Loader2,
  Lock,
  Pencil,
  RefreshCw,
  Search,
  Settings2,
  ShieldCheck,
  Trash2,
  Upload,
  X,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import {
  type FileEntry,
  type FileRoot,
  copyEntry,
  deleteEntry,
  deleteRoot,
  downloadUrl,
  hashFile,
  joinPath,
  mkdir,
  moveEntry,
  parentPath,
  rootsKey,
  archiveUrl,
  bulk,
  isArchive,
  type BulkResult,
  useDirectory,
  useFileRoots,
  uniqueRoots,
} from "../api/files";
import { errorText, useToast } from "../app/toast";
import { FileViewer } from "../components/files/FileViewer";
import { AccessPanel } from "../components/files/AccessPanel";
import { BulkDialog, BulkResults, CompressDialog, ExtractDialog, PermissionsDialog } from "../components/files/ManageDialogs";
import { RootsDialog, SearchDrawer, TrashDrawer } from "../components/files/FilePanels";
import { UploadDialog, filesFromDrop, queueSummary, useUploadQueue } from "../components/files/UploadDialog";
import { Badge, Button, Confirm, Empty, ErrorNote, Field, IconButton, Input, Modal, PointerMenu, type PointerMenuState, RowMenu, type RowAction, Select, menuPoint, Skeleton, Table, Td, bytes, time } from "../components/ui";
import { COPY_BLOCKED, copyText } from "../lib/clipboard";

const mode = (m?: number) =>
  m === undefined ? "" : [6, 3, 0].map((s) => ((m >> s) & 4 ? "r" : "-") + ((m >> s) & 2 ? "w" : "-") + ((m >> s) & 1 ? "x" : "-")).join("");

type Dialog =
  | { kind: "mkdir" | "newfile" }
  | { kind: "rename" | "copy" | "move"; entry: FileEntry }
  | { kind: "delete"; entry: FileEntry; permanent: boolean };

const ROOT_GROUPS: [FileRoot["kind"], string][] = [
  ["node", "Node"],
  ["service", "Services"],
  ["container", "Containers"],
];
const rootTitle = (r: FileRoot) => `${r.label} — ${r.container ? `${r.container.slice(0, 12)}:` : ""}${r.base}${r.read_only ? " (read-only)" : ""}`;
/** Option text: the label, plus the path's tail; same label and path twice gets the id too. */
function rootOption(r: FileRoot, all: FileRoot[]): string {
  if (r.kind === "node") return r.label;
  const tail = r.base.length > 32 ? `…${r.base.slice(-31)}` : r.base;
  const twin = all.some((o) => o.id !== r.id && o.label === r.label && o.base === r.base);
  return `${r.label} · ${tail}${twin ? ` · ${r.id.slice(0, 7)}` : ""}`;
}

/** File explorer for one node: roots, browse, view, edit, upload and manage files. */
export function FileExplorer({ serverId, disabled, initialRoot }: { serverId: string; disabled?: boolean; initialRoot?: string }) {
  const roots = useFileRoots(serverId);
  const [rootId, setRootId] = useState<string | null>(initialRoot ?? null);
  // Unique by id: a duplicated root from the server never breaks the list.
  const rootList = useMemo(() => uniqueRoots(roots.data?.roots), [roots.data]);
  const root: FileRoot | undefined = rootList.find((r) => r.id === rootId) ?? rootList[0];
  const [removingRoot, setRemovingRoot] = useState<FileRoot | null>(null);
  const [path, setPath] = useState("/");
  const [draft, setDraft] = useState("/");
  const [hidden, setHidden] = useState(false);
  const [open, setOpen] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  // Multi-selection (names in the current folder) and the last clicked row.
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const anchor = useRef<string | null>(null);
  const [manage, setManage] = useState<
    | { kind: "perm" | "compress" | "copy" | "move"; entries: FileEntry[] }
    | { kind: "extract"; path: string }
    | { kind: "results"; result: BulkResult }
    | null
  >(null);
  const [dialog, setDialog] = useState<Dialog | null>(null);
  const [menu, setMenu] = useState<PointerMenuState | null>(null);
  const [panel, setPanel] = useState<"trash" | "search" | "roots" | "access" | null>(null);
  const [uploadOpen, setUploadOpen] = useState(false);
  const [dragging, setDragging] = useState(false);
  const dirRef = useRef<HTMLInputElement>(null);
  const qc = useQueryClient();
  const toast = useToast();
  const queue = useUploadQueue(serverId, () => void qc.invalidateQueries({ queryKey: ["files", serverId] }));
  const dir = useDirectory(serverId, root?.id ?? "node", path, hidden, !!root);

  useEffect(() => setDraft(path), [path]);
  useEffect(() => {
    setPath("/");
    setOpen(null);
  }, [root?.id]);

  const pages = dir.data?.pages ?? [];
  const entries = useMemo(() => pages.flatMap((p) => p.entries), [pages]);
  const last = pages[pages.length - 1];
  const crumbs = path.split("/").filter(Boolean);
  const windows = roots.data?.os === "windows";
  const canWrite = !!root && !root.read_only && root.permissions.write;
  const canDelete = !!root && !root.read_only && root.permissions.delete;
  const refresh = () => void qc.invalidateQueries({ queryKey: ["files", serverId] });

  const go = (p: string) => {
    setPath(p || "/");
    setOpen(null);
    setSelected(null);
    setChecked(new Set());
  };

  /** Click with Ctrl/Cmd toggles, Shift selects a range, plain selects one. */
  const pick = (ev: React.MouseEvent, name: string) => {
    if (ev.shiftKey && anchor.current) {
      const names = entries.map((x) => x.name);
      const [a, b] = [names.indexOf(anchor.current), names.indexOf(name)].sort((x, y) => x - y);
      if (a !== undefined && b !== undefined && a >= 0) {
        setChecked(new Set(names.slice(a, b + 1)));
        return;
      }
    }
    if (ev.ctrlKey || ev.metaKey) {
      setChecked((c) => {
        const n = new Set(c);
        if (n.has(name)) n.delete(name);
        else n.add(name);
        return n;
      });
    } else {
      setChecked(new Set());
    }
    anchor.current = name;
    setSelected(name);
  };

  // F2 renames, Delete trashes the selected row.
  useEffect(() => {
    const onKey = (ev: KeyboardEvent) => {
      if (!selected || dialog || panel || (ev.target as HTMLElement)?.closest("input,textarea,.cm-editor")) return;
      const e = entries.find((x) => x.name === selected);
      if (!e || e.protected) return;
      if (ev.key === "F2" && canWrite && canDelete) setDialog({ kind: "rename", entry: e });
      if (ev.key === "Delete" && canDelete) setDialog({ kind: "delete", entry: e, permanent: ev.shiftKey });
      if (ev.key === "a" && (ev.ctrlKey || ev.metaKey)) {
        ev.preventDefault();
        setChecked(new Set(entries.filter((x) => !x.protected).map((x) => x.name)));
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selected, dialog, panel, entries, canWrite, canDelete]);

  if (disabled) return <Empty title="Files are unavailable">This node is revoked.</Empty>;
  if (roots.isLoading) return <Skeleton rows={6} />;
  if (roots.error) return <ErrorNote error={roots.error} />;
  if (!root) {
    return (
      <Empty title="No file roots">
        You can't open any directory on this node. An administrator can approve service directories for you.
      </Empty>
    );
  }

  /** Uploads files, creating folders for their relative paths first. */
  const upload = async (items: { file: File; rel: string }[]) => {
    if (!canWrite || !items.length) return;
    setUploadOpen(true);
    const dirs = new Set<string>();
    for (const { rel } of items) {
      const parts = rel.split("/").slice(0, -1);
      for (let i = 1; i <= parts.length; i++) dirs.add(parts.slice(0, i).join("/"));
    }
    for (const d of [...dirs].sort((a, b) => a.length - b.length)) {
      await mkdir(serverId, root.id, joinPath(path, d)).catch(() => undefined);
    }
    queue.add(items.map(({ file, rel }) => ({ file, name: rel, root: root.id, dest: joinPath(path, rel) })));
  };

  const onDrop = async (ev: React.DragEvent) => {
    ev.preventDefault();
    setDragging(false);
    if (!canWrite) return;
    void upload(await filesFromDrop(ev.dataTransfer));
  };

  const rowActions = (e: FileEntry) => {
    const p = joinPath(path, e.name);
    return [
      { label: "Open", icon: e.kind === "dir" ? Folder : Eye, onSelect: () => (e.kind === "dir" ? go(p) : setOpen(p)), hidden: e.kind === "symlink" || e.kind === "other" },
      { label: "Download", icon: Download, onSelect: () => window.open(downloadUrl(serverId, root.id, p), "_self"), hidden: e.kind !== "file" },
      { label: "Rename (F2)", icon: Pencil, onSelect: () => setDialog({ kind: "rename", entry: e }), hidden: !canWrite || !canDelete },
      { label: "Copy to…", icon: CopyPlus, onSelect: () => setDialog({ kind: "copy", entry: e }), hidden: !canWrite },
      { label: "Move to…", icon: FolderInput, onSelect: () => setDialog({ kind: "move", entry: e }), hidden: !canWrite || !canDelete },
      {
        label: "SHA-256",
        icon: Fingerprint,
        hidden: e.kind !== "file",
        onSelect: () =>
          void hashFile(serverId, root.id, p)
            .then((h) => {
              void copyText(h.sha256).then((ok) => (ok ? toast(`SHA-256 copied: ${h.sha256.slice(0, 16)}…`, "ok") : toast(COPY_BLOCKED, "error")));
            })
            .catch((err) => toast(errorText(err), "error")),
      },
      { label: "Copy path", icon: Copy, onSelect: () => void copyText(p).then((ok) => (ok ? toast("Path copied") : toast(COPY_BLOCKED, "error"))) },
      { label: "Compress…", icon: FileArchive, onSelect: () => setManage({ kind: "compress", entries: [e] }), hidden: !canWrite },
      { label: "Extract…", icon: FolderInput, onSelect: () => setManage({ kind: "extract", path: p }), hidden: !canWrite || e.kind !== "file" || !isArchive(e.name) },
      { label: "Permissions…", icon: ShieldCheck, onSelect: () => setManage({ kind: "perm", entries: [e] }), hidden: !root.permissions.admin && !windows },
      { label: "Move to trash", icon: Trash2, danger: true, onSelect: () => setDialog({ kind: "delete", entry: e, permanent: false }), hidden: !canDelete || root.kind === "container" },
      { label: "Delete permanently", icon: Trash2, danger: true, onSelect: () => setDialog({ kind: "delete", entry: e, permanent: true }), hidden: !canDelete },
    ];
  };

  const chosen = entries.filter((x) => checked.has(x.name));
  const done = (msg: string) => {
    setManage(null);
    setChecked(new Set());
    refresh();
    toast(msg, "ok");
  };
  const downloadChosen = async () => {
    try {
      const one = chosen.length === 1 ? chosen[0] : undefined;
      const url =
        one && one.kind === "file"
          ? downloadUrl(serverId, root.id, joinPath(path, one.name))
          : await archiveUrl(serverId, root.id, chosen.map((x) => joinPath(path, x.name)));
      window.open(url, "_self");
    } catch (err) {
      toast(errorText(err), "error");
    }
  };
  /** Actions on the whole multi-selection: the selection bar and its right-click menu. */
  const bulkActions = (sel: FileEntry[]): RowAction[] => [
    { label: "Download", icon: Download, onSelect: () => void downloadChosen() },
    { label: "Copy to…", icon: CopyPlus, onSelect: () => setManage({ kind: "copy", entries: sel }), hidden: !canWrite },
    { label: "Move to…", icon: FolderInput, onSelect: () => setManage({ kind: "move", entries: sel }), hidden: !canWrite || !canDelete },
    { label: "Compress…", icon: FileArchive, onSelect: () => setManage({ kind: "compress", entries: sel }), hidden: !canWrite },
    { label: "Permissions…", icon: ShieldCheck, onSelect: () => setManage({ kind: "perm", entries: sel }), hidden: !root.permissions.admin || windows },
    { label: "Move to trash", icon: Trash2, danger: true, onSelect: () => void bulkDelete(false), hidden: !canDelete || root.kind === "container" },
    {
      label: "Delete permanently",
      icon: Trash2,
      danger: true,
      onSelect: () => window.confirm(`Delete ${sel.length} item(s) for good?`) && void bulkDelete(true),
      hidden: !canDelete,
    },
  ];
  /** Folder-level actions for a right-click on empty space in the listing. */
  const folderActions: RowAction[] = [
    { label: "New folder", icon: FolderPlus, onSelect: () => setDialog({ kind: "mkdir" }), hidden: !canWrite },
    { label: "New file", icon: FilePlus, onSelect: () => setDialog({ kind: "newfile" }), hidden: !canWrite },
    { label: "Upload files…", icon: Upload, onSelect: () => setUploadOpen(true), hidden: !canWrite },
    { label: "Upload folder…", icon: FolderUp, onSelect: () => dirRef.current?.click(), hidden: !canWrite },
    { label: "Copy path", icon: Copy, onSelect: () => void copyText(path).then((ok) => (ok ? toast("Path copied") : toast(COPY_BLOCKED, "error"))) },
    { label: hidden ? "Hide hidden files" : "Show hidden files", icon: hidden ? EyeOff : Eye, onSelect: () => setHidden((h) => !h) },
    { label: "Refresh", icon: RefreshCw, onSelect: refresh },
  ];
  /** Right-click on a row: select it first unless it is part of a multi-selection. */
  const rowContext = (ev: React.MouseEvent | React.KeyboardEvent, e: FileEntry) => {
    ev.preventDefault();
    ev.stopPropagation();
    if (e.protected) return;
    const at = menuPoint(ev as React.MouseEvent<Element>);
    if (checked.size > 1 && checked.has(e.name)) {
      setMenu({ ...at, actions: bulkActions(chosen), label: `Actions for ${chosen.length} items` });
      return;
    }
    setChecked(new Set());
    anchor.current = e.name;
    setSelected(e.name);
    setMenu({ ...at, actions: rowActions(e), label: `Actions for ${e.name}` });
  };
  const bulkDelete = async (permanent: boolean) => {
    try {
      const r = await bulk(serverId, root.id, permanent ? "delete" : "trash", chosen.map((x) => joinPath(path, x.name)));
      setChecked(new Set());
      refresh();
      if (r.failed) setManage({ kind: "results", result: r });
      else toast(permanent ? `Deleted ${r.ok} item(s)` : `Moved ${r.ok} item(s) to the trash`, "ok");
    } catch (err) {
      toast(errorText(err), "error");
    }
  };

  return (
    <div className="min-w-0 rounded-lg border border-line bg-panel">
      <div className="flex flex-wrap items-center gap-2 border-b border-line px-3 py-2 text-xs">
        {rootList.length > 1 ? (
          <Select aria-label="Root" className="!w-64" title={rootTitle(root)} value={root.id} onChange={(e) => setRootId(e.target.value)}>
            {ROOT_GROUPS.map(([kind, groupLabel]) => {
              const group = rootList.filter((r) => r.kind === kind);
              if (!group.length) return null;
              return (
                <optgroup key={kind} label={groupLabel}>
                  {group.map((r) => (
                    <option key={r.id} value={r.id} title={rootTitle(r)}>
                      {rootOption(r, rootList)}
                    </option>
                  ))}
                </optgroup>
              );
            })}
          </Select>
        ) : (
          <Badge tone="muted">{root.label}</Badge>
        )}
        {root.kind !== "node" && (
          <span className="max-w-[28rem] truncate font-mono text-faint" title={rootTitle(root)}>
            {root.container ? `${root.container.slice(0, 12)}:` : ""}
            {root.base}
          </span>
        )}
        <Badge tone="muted">{root.kind}</Badge>
        {root.kind !== "node" && root.permissions.admin && (
          <IconButton label="Remove root" title="Remove this root (the files stay on the node)" onClick={() => setRemovingRoot(root)}>
            <Trash2 className="size-3.5" />
          </IconButton>
        )}
        {root.read_only ? <Badge tone="warn">read-only</Badge> : !root.permissions.write && <Badge tone="muted">view only</Badge>}
        {root.container && <Badge tone="muted">container {root.container.slice(0, 12)}</Badge>}
        {root.kind === "container" && <span className="text-warn">Changes live in the container layer and are lost when it is recreated.</span>}
        {canWrite && !roots.data?.mfa && <span className="text-faint">Changes ask for an MFA check.</span>}
        <span className="ml-auto flex gap-1">
          <Button variant="ghost" onClick={() => setPanel("search")}>
            <Search className="size-4" /> Search
          </Button>
          {root.kind !== "container" && (
            <Button variant="ghost" onClick={() => setPanel("trash")}>
              <Trash2 className="size-4" /> Trash
            </Button>
          )}
          <Button variant="ghost" onClick={() => setPanel("access")} title="Create SFTP Access">
            <KeyRound className="size-4" /> SFTP access
          </Button>
          {root.permissions.admin && (
            <Button variant="ghost" onClick={() => setPanel("roots")}>
              <Settings2 className="size-4" /> Roots
            </Button>
          )}
        </span>
      </div>

      <div className="flex flex-wrap items-center gap-2 border-b border-line px-3 py-2">
        <IconButton label="Up one level" title={crumbs.length === 0 ? "Already at the top of this root" : "Up one level"} disabled={crumbs.length === 0} onClick={() => go(parentPath(path))}>
          <ArrowUp className="size-4" />
        </IconButton>
        <nav aria-label="Path" className="flex min-w-0 flex-wrap items-center gap-0.5 font-mono text-xs">
          <button className="rounded px-1 hover:bg-panel-2 hover:text-accent" onClick={() => go("/")} title="Root">
            {windows && root.kind === "node" ? <HardDrive className="size-3.5" /> : root.kind === "node" ? "/" : root.label}
          </button>
          {crumbs.map((c, i) => (
            <span key={i} className="flex min-w-0 items-center gap-0.5">
              <ChevronRight className="size-3 shrink-0 text-faint" />
              <button className="max-w-[12rem] truncate rounded px-1 hover:bg-panel-2 hover:text-accent" title={c} onClick={() => go("/" + crumbs.slice(0, i + 1).join("/"))}>
                {c}
              </button>
            </span>
          ))}
        </nav>
        <form
          className="ml-auto flex items-center gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            const t = draft.trim();
            go(t.startsWith("/") ? t : `/${t}`);
          }}
        >
          <Input className="!w-56 font-mono !text-xs" aria-label="Go to path" placeholder={windows ? "/C:/Users" : "/var/log"} value={draft} onChange={(e) => setDraft(e.target.value)} />
        </form>
        <IconButton label={hidden ? "Hide hidden files" : "Show hidden files"} onClick={() => setHidden((h) => !h)}>
          {hidden ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
        </IconButton>
        <IconButton label="Refresh" onClick={refresh}>
          <RefreshCw className={`size-4 ${dir.isFetching ? "animate-spin" : ""}`} />
        </IconButton>
        {canWrite && (
          <>
            <IconButton label="New folder" onClick={() => setDialog({ kind: "mkdir" })}>
              <FolderPlus className="size-4" />
            </IconButton>
            <IconButton label="New file" onClick={() => setDialog({ kind: "newfile" })}>
              <FilePlus className="size-4" />
            </IconButton>
            <input
              ref={dirRef}
              type="file"
              className="hidden"
              aria-label="Folder to upload"
              // @ts-expect-error: non-standard but supported by every current browser
              webkitdirectory=""
              onChange={(e) => {
                void upload(Array.from(e.target.files ?? []).map((file) => ({ file, rel: file.webkitRelativePath || file.name })));
                e.target.value = "";
              }}
            />
            <IconButton label="Upload folder" onClick={() => dirRef.current?.click()}>
              <FolderUp className="size-4" />
            </IconButton>
<UploadIndicator queue={queue} onOpen={() => setUploadOpen(true)} />
            <Button onClick={() => setUploadOpen(true)}>
              <Upload className="size-4" /> Upload
            </Button>
          </>
        )}
      </div>

      {chosen.length > 0 && (
        <div className="flex flex-wrap items-center gap-2 border-b border-line bg-accent-soft px-3 py-1.5 text-xs" role="toolbar" aria-label="Selection">
          <span className="font-medium">
            {chosen.length} selected · {bytes(chosen.reduce((n, x) => n + (x.kind === "file" ? x.size : 0), 0))}
          </span>
          {bulkActions(chosen)
            .filter((a) => !a.hidden)
            .map((a) => (
              <Button key={a.label} variant="ghost" onClick={a.onSelect}>
                {a.icon && <a.icon className={`size-4 ${a.danger ? "text-bad" : ""}`} />} {a.label}
              </Button>
            ))}
          <Button variant="ghost" className="ml-auto" onClick={() => setChecked(new Set())}>
            Clear
          </Button>
        </div>
      )}

      <div className={`grid min-w-0 ${open ? "lg:grid-cols-2" : ""}`}>
        <div
          className={`min-w-0 overflow-x-auto ${dragging ? "outline-2 -outline-offset-2 outline-dashed outline-accent" : ""}`}
          onDragOver={(e) => {
            if (!canWrite) return;
            e.preventDefault();
            setDragging(true);
          }}
          onDragLeave={() => setDragging(false)}
          onDrop={(e) => void onDrop(e)}
          onContextMenu={(ev) => {
            // Leave the native menu in text fields; everywhere else show the folder menu.
            if ((ev.target as HTMLElement)?.closest("input,textarea,.cm-editor")) return;
            ev.preventDefault();
            setMenu({ ...menuPoint(ev), actions: folderActions, label: `Actions for ${path}` });
          }}
        >
          {dir.isLoading ? (
            <div className="p-3">
              <Skeleton rows={8} />
            </div>
          ) : dir.error ? (
            <div className="p-3">
              <ErrorNote error={dir.error} />
            </div>
          ) : entries.length === 0 ? (
            <Empty icon={<Folder className="size-5" />} title="Empty directory">
              {canWrite ? "Drop files or folders here to upload them." : undefined}
            </Empty>
          ) : (
            <Table
              head={[
                <input
                  key="all"
                  type="checkbox"
                  aria-label="Select all"
                  checked={checked.size > 0 && checked.size === entries.filter((x) => !x.protected).length}
                  onChange={(ev) => setChecked(ev.target.checked ? new Set(entries.filter((x) => !x.protected).map((x) => x.name)) : new Set())}
                />,
                "Name",
                "Size",
                "Modified",
                windows ? "Attributes" : "Mode",
                windows ? "" : "Owner",
                "",
              ]}
            >
              {entries.map((e) => {
                const p = joinPath(path, e.name);
                const clickable = !e.protected && (e.kind === "dir" || e.kind === "file");
                return (
                  <tr
                    key={e.name}
                    onClick={(ev) => pick(ev, e.name)}
                    onContextMenu={(ev) => rowContext(ev, e)}
                    onKeyDown={(ev) => {
                      if (ev.key === "ContextMenu" || (ev.key === "F10" && ev.shiftKey)) rowContext(ev, e);
                    }}
                    tabIndex={e.protected ? undefined : 0}
                    className={`${open === p || selected === e.name || checked.has(e.name) ? "bg-panel-2" : ""} ${e.protected ? "opacity-60" : ""}`}
                    aria-selected={selected === e.name || checked.has(e.name)}
                  >
                    <Td className="w-6">
                      {!e.protected && (
                        <input
                          type="checkbox"
                          aria-label={`Select ${e.name}`}
                          checked={checked.has(e.name)}
                          onClick={(ev) => ev.stopPropagation()}
                          onChange={() =>
                            setChecked((c) => {
                              const n = new Set(c);
                              if (n.has(e.name)) n.delete(e.name);
                              else n.add(e.name);
                              return n;
                            })
                          }
                        />
                      )}
                    </Td>
                    <Td>
                      <button
                        className={`flex max-w-[28rem] items-center gap-2 truncate text-left font-mono text-xs ${clickable ? "hover:text-accent" : "cursor-default"}`}
                        onDoubleClick={() => clickable && (e.kind === "dir" ? go(p) : setOpen(p))}
                        onClick={() => clickable && (e.kind === "dir" ? go(p) : setOpen(p))}
                        title={e.protected ? "Protected: Forge does not open this path" : e.link_target ? `${e.name} → ${e.link_target}` : e.name}
                      >
                        {e.kind === "dir" ? <Folder className="size-4 shrink-0 text-accent" /> : e.kind === "symlink" ? <Link2 className="size-4 shrink-0 text-faint" /> : <FileIcon className="size-4 shrink-0 text-faint" />}
                        <span className="truncate">{e.name}</span>
                        {e.link_target && <span className="truncate text-faint">→ {e.link_target}</span>}
                        {e.protected && <Lock className="size-3 shrink-0 text-warn" aria-label="protected" />}
                        {e.mount && <Badge tone="muted">mount</Badge>}
                      </button>
                    </Td>
                    <Td className="font-mono text-xs tabular-nums">{e.kind === "file" ? bytes(e.size) : ""}</Td>
                    <Td className="text-xs text-faint">{e.modified_ms ? time(e.modified_ms) : ""}</Td>
                    <Td className="font-mono text-xs">{windows ? [e.readonly && "R", e.hidden && "H"].filter(Boolean).join(" ") : mode(e.mode)}</Td>
                    <Td className="font-mono text-xs text-faint">{windows ? "" : `${e.owner ?? ""}:${e.group ?? ""}`}</Td>
                    <Td className="text-right">{!e.protected && <RowMenu actions={rowActions(e)} label={`Actions for ${e.name}`} />}</Td>
                  </tr>
                );
              })}
            </Table>
          )}
          <div className="flex items-center gap-3 border-t border-line px-3 py-2 text-xs text-faint">
            <span>
              {entries.length} of {last?.total ?? 0} entries
              {last?.truncated ? " (directory too large, listing truncated)" : ""}
              {last?.skipped_names ? ` · ${last.skipped_names} names are not valid UTF-8 and are not shown` : ""}
            </span>
            {dir.hasNextPage && (
              <Button variant="ghost" onClick={() => void dir.fetchNextPage()} disabled={dir.isFetchingNextPage}>
                {dir.isFetchingNextPage && <Loader2 className="size-4 animate-spin" />} Load more
              </Button>
            )}
          </div>
        </div>
        {open && <FileViewer key={open} serverId={serverId} root={root} path={open} onClose={() => setOpen(null)} onSaved={refresh} />}
      </div>

      <UploadDialog open={uploadOpen} onOpenChange={setUploadOpen} dir={path} queue={queue} onFiles={(items) => void upload(items)} onFolder={() => dirRef.current?.click()} />
      <PointerMenu menu={menu} onClose={() => setMenu(null)} />
      {manage?.kind === "perm" && (
        <PermissionsDialog serverId={serverId} root={root} dir={path} entries={manage.entries} windows={windows} onClose={() => setManage(null)} onDone={done} />
      )}
      {manage?.kind === "compress" && <CompressDialog serverId={serverId} root={root} dir={path} entries={manage.entries} windows={windows} existing={entries.map((x) => x.name)} onClose={() => setManage(null)} onDone={done} />}
      {manage?.kind === "extract" && <ExtractDialog serverId={serverId} root={root} path={manage.path} onClose={() => setManage(null)} onDone={done} />}
      {(manage?.kind === "copy" || manage?.kind === "move") && (
        <BulkDialog serverId={serverId} root={root} dir={path} entries={manage.entries} action={manage.kind} onClose={() => setManage(null)} onDone={done} />
      )}
      {manage?.kind === "results" && (
        <Modal open onOpenChange={(o) => !o && setManage(null)} title="Results">
          <BulkResults result={manage.result} onClose={() => setManage(null)} />
        </Modal>
      )}
      {dialog && (
        <EntryDialog
          dialog={dialog}
          dir={path}
          onClose={() => setDialog(null)}
          onDone={(msg, newOpen) => {
            setDialog(null);
            refresh();
            toast(msg, "ok");
            if (newOpen) setOpen(newOpen);
          }}
          serverId={serverId}
          root={root}
        />
      )}
      {panel === "trash" && <TrashDrawer serverId={serverId} root={root} onClose={() => setPanel(null)} onChanged={refresh} />}
      {panel === "search" && <SearchDrawer serverId={serverId} root={root} start={path} onClose={() => setPanel(null)} onOpen={(p) => (go(parentPath(p)), setOpen(p), setPanel(null))} />}
      <Confirm
        open={!!removingRoot}
        onOpenChange={(o) => !o && setRemovingRoot(null)}
        title={`Remove root ${removingRoot?.label ?? ""}?`}
        body={`Users can no longer browse ${removingRoot?.base ?? "this directory"} through this root. The files stay on the node.`}
        confirmLabel="Remove root"
        onConfirm={() => {
          const r = removingRoot;
          setRemovingRoot(null);
          if (!r) return;
          deleteRoot(serverId, r.id).then(
            () => {
              toast("Root removed", "ok");
              setRootId(null);
              setPath("/");
              void qc.invalidateQueries({ queryKey: rootsKey(serverId) });
            },
            (e) => toast(errorText(e), "error"),
          );
        }}
      />
      {panel === "access" && <AccessPanel serverId={serverId} roots={roots.data?.roots ?? []} current={root} onClose={() => setPanel(null)} />}
      {panel === "roots" && (
        <RootsDialog serverId={serverId} roots={roots.data?.roots ?? []} os={roots.data?.os ?? "linux"} onClose={() => setPanel(null)} onChanged={() => void qc.invalidateQueries({ queryKey: rootsKey(serverId) })} />
      )}
    </div>
  );
}

/** New folder / new file / rename / copy / move / delete. */
function EntryDialog({
  dialog,
  dir,
  serverId,
  root,
  onClose,
  onDone,
}: {
  dialog: Dialog;
  dir: string;
  serverId: string;
  root: FileRoot;
  onClose: () => void;
  onDone: (msg: string, open?: string) => void;
}) {
  const entry = "entry" in dialog ? dialog.entry : undefined;
  const from = entry ? joinPath(dir, entry.name) : "";
  const initial = dialog.kind === "rename" ? entry?.name ?? "" : dialog.kind === "copy" || dialog.kind === "move" ? from : "";
  const [value, setValue] = useState(initial);
  const [overwrite, setOverwrite] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = async (f: () => Promise<unknown>, msg: string, open?: string) => {
    setBusy(true);
    setError(null);
    try {
      await f();
      onDone(msg, open);
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };

  if (dialog.kind === "delete") {
    return (
      <Confirm
        open
        onOpenChange={(o) => !o && onClose()}
        title={dialog.permanent ? "Delete permanently?" : "Move to trash?"}
        body={
          <>
            <span className="font-mono">{from}</span>{" "}
            {dialog.permanent ? "and everything in it will be removed for good." : "can be restored from the trash."}
            {error && <p className="mt-2 text-bad">{error}</p>}
          </>
        }
        confirmLabel={dialog.permanent ? "Delete" : "Move to trash"}
        busy={busy}
        onConfirm={() => void run(() => deleteEntry(serverId, root.id, from, dialog.permanent), dialog.permanent ? "Deleted" : "Moved to trash")}
      />
    );
  }

  const titles = { mkdir: "New folder", newfile: "New file", rename: "Rename", copy: "Copy to", move: "Move to" } as const;
  const submit = () => {
    const v = value.trim();
    if (!v) return;
    switch (dialog.kind) {
      case "mkdir":
        return void run(() => mkdir(serverId, root.id, joinPath(dir, v)), "Folder created");
      case "newfile":
        // An empty new file opens in the editor; it is created on first save.
        return onDone("Edit the new file and save it", joinPath(dir, v));
      case "rename":
        return void run(() => moveEntry(serverId, root.id, from, joinPath(dir, v), overwrite), "Renamed");
      case "copy":
        return void run(() => copyEntry(serverId, root.id, from, v, overwrite), "Copied");
      case "move":
        return void run(() => moveEntry(serverId, root.id, from, v, overwrite), "Moved");
    }
  };

  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title={titles[dialog.kind]} description={entry ? from : `In ${dir}`}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
      >
        <Field label={dialog.kind === "copy" || dialog.kind === "move" ? "Destination path" : "Name"}>
          <Input autoFocus className="font-mono" value={value} onChange={(e) => setValue(e.target.value)} />
        </Field>
        {(dialog.kind === "rename" || dialog.kind === "copy" || dialog.kind === "move") && (
          <label className="mt-3 flex items-center gap-2 text-xs text-muted">
            <input type="checkbox" checked={overwrite} onChange={(e) => setOverwrite(e.target.checked)} /> Replace the destination if it exists
          </label>
        )}
        {error && <p className="mt-3 text-xs text-bad">{error}</p>}
        <div className="mt-5 flex justify-end gap-2">
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" disabled={busy || !value.trim() || value === initial}>
            {busy && <Loader2 className="size-4 animate-spin" />} {titles[dialog.kind]}
          </Button>
        </div>
      </form>
    </Modal>
  );
}

/** Toolbar chip while uploads run or failed; reopens the upload dialog. */
function UploadIndicator({ queue, onOpen }: { queue: ReturnType<typeof useUploadQueue>; onOpen: () => void }) {
  const { pending, failed, pct } = queueSummary(queue.items);
  if (!pending && !failed) return null;
  return (
    <button
      type="button"
      onClick={onOpen}
      title="Show uploads"
      className={`inline-flex h-7 items-center gap-1.5 rounded-md border px-2 font-mono text-[11px] tabular-nums transition hover:bg-panel-2 focus-visible:ring-2 focus-visible:ring-accent/50 focus-visible:outline-none ${failed && !pending ? "border-bad text-bad" : "border-line text-muted"}`}
    >
      {pending ? <Loader2 className="size-3.5 animate-spin text-accent" /> : <X className="size-3.5" />}
      {pending ? `${pending} uploading · ${pct}%` : ""}
      {failed ? `${pending ? " · " : ""}${failed} failed` : ""}
    </button>
  );
}
