import { CheckCircle2, CircleAlert, FolderUp, Loader2, RotateCcw, Upload, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import { uploadFile } from "../../api/files";
import { errorText } from "../../app/toast";
import { bytesRate } from "../Traffic";
import { Button, IconButton, Modal, bytes } from "../ui";

/** Files uploading at once; each one already sends several chunks in parallel. */
const CONCURRENT_FILES = 2;
/** Window for the speed estimate, in ms. */
const SPEED_WINDOW = 5000;

export type UploadStatus = "queued" | "uploading" | "verifying" | "done" | "failed" | "cancelled";

export interface UploadItem {
  id: number;
  /** Name shown in the list (relative path for folder uploads). */
  name: string;
  root: string;
  /** Destination path inside the root. */
  dest: string;
  file: File;
  status: UploadStatus;
  sent: number;
  total: number;
  /** Bytes per second over the last few seconds; 0 when unknown. */
  speed: number;
  error?: string;
}

export interface UploadRequest {
  file: File;
  name: string;
  root: string;
  dest: string;
}

let seq = 0;
const active = (s: UploadStatus) => s === "uploading" || s === "verifying";

/**
 * The upload queue of one file explorer. Files start as soon as they are
 * added; at most `CONCURRENT_FILES` upload at a time. Progress comes from the
 * chunk completions reported by `uploadFile`. Uploads still running when the
 * explorer unmounts are cancelled.
 */
export function useUploadQueue(serverId: string, onFinished: () => void) {
  const [items, setItems] = useState<UploadItem[]>([]);
  const controllers = useRef(new Map<number, AbortController>());
  const finished = useRef(onFinished);
  finished.current = onFinished;

  const patch = useCallback((id: number, p: Partial<UploadItem>) => setItems((xs) => xs.map((x) => (x.id === id ? { ...x, ...p } : x))), []);

  const run = useCallback(
    async (it: UploadItem) => {
      const ac = new AbortController();
      controllers.current.set(it.id, ac);
      const samples: { t: number; sent: number }[] = [{ t: performance.now(), sent: 0 }];
      try {
        await uploadFile(
          serverId,
          it.root,
          it.dest,
          it.file,
          "never",
          (sent, total) => {
            const t = performance.now();
            samples.push({ t, sent });
            while (samples.length > 2 && t - (samples[0]?.t ?? t) > SPEED_WINDOW) samples.shift();
            const first = samples[0];
            const speed = first && t > first.t ? ((sent - first.sent) * 1000) / (t - first.t) : 0;
            // All bytes are on the node: the commit checks size and SHA-256.
            patch(it.id, { sent, total, speed, status: sent >= total ? "verifying" : "uploading" });
          },
          ac.signal,
        );
        patch(it.id, { status: "done", sent: it.file.size, speed: 0 });
        finished.current();
      } catch (e) {
        patch(it.id, ac.signal.aborted ? { status: "cancelled", speed: 0 } : { status: "failed", speed: 0, error: errorText(e) });
      } finally {
        controllers.current.delete(it.id);
      }
    },
    [serverId, patch],
  );

  // Start queued files while there is room.
  useEffect(() => {
    let running = items.filter((x) => active(x.status)).length;
    const start: UploadItem[] = [];
    for (const x of items) {
      if (running >= CONCURRENT_FILES) break;
      if (x.status === "queued") {
        start.push(x);
        running++;
      }
    }
    if (!start.length) return;
    const ids = new Set(start.map((x) => x.id));
    setItems((xs) => xs.map((x) => (ids.has(x.id) ? { ...x, status: "uploading" } : x)));
    for (const x of start) void run(x);
  }, [items, run]);

  // Cancel on unmount; warn before leaving the page with uploads running.
  useEffect(() => {
    const map = controllers.current;
    return () => map.forEach((c) => c.abort());
  }, []);
  const busy = items.some((x) => active(x.status) || x.status === "queued");
  useEffect(() => {
    if (!busy) return;
    const warn = (e: BeforeUnloadEvent) => e.preventDefault();
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [busy]);

  const add = useCallback((reqs: UploadRequest[]) => {
    if (!reqs.length) return;
    setItems((xs) => [...xs, ...reqs.map((r): UploadItem => ({ ...r, id: ++seq, status: "queued", sent: 0, total: r.file.size, speed: 0 }))]);
  }, []);
  const cancel = useCallback(
    (id: number) => {
      const c = controllers.current.get(id);
      if (c) c.abort();
      else patch(id, { status: "cancelled" });
    },
    [patch],
  );
  const retry = useCallback((id: number) => patch(id, { status: "queued", sent: 0, speed: 0, error: undefined }), [patch]);
  const dismiss = useCallback((id: number) => setItems((xs) => xs.filter((x) => x.id !== id)), []);
  const clearFinished = useCallback(() => setItems((xs) => xs.filter((x) => x.status !== "done" && x.status !== "cancelled")), []);

  return { items, busy, add, cancel, retry, dismiss, clearFinished };
}

export type UploadQueue = ReturnType<typeof useUploadQueue>;

/** Files (with relative paths for folders) from a drop; empty when nothing usable was dropped. */
export async function filesFromDrop(dt: DataTransfer): Promise<{ file: File; rel: string }[]> {
  const out: { file: File; rel: string }[] = [];
  // Entries must be taken synchronously, before the first await.
  const entries = Array.from(dt.items ?? [])
    .map((i) => i.webkitGetAsEntry?.())
    .filter((e): e is FileSystemEntry => !!e);
  const files = Array.from(dt.files ?? []);
  const walk = async (entry: FileSystemEntry, prefix: string): Promise<void> => {
    if (entry.isFile) {
      const file = await new Promise<File>((res, rej) => (entry as FileSystemFileEntry).file(res, rej));
      out.push({ file, rel: prefix + entry.name });
    } else if (entry.isDirectory) {
      const reader = (entry as FileSystemDirectoryEntry).createReader();
      let batch: FileSystemEntry[] = [];
      do {
        batch = await new Promise<FileSystemEntry[]>((res, rej) => reader.readEntries(res, rej));
        for (const child of batch) await walk(child, `${prefix}${entry.name}/`);
      } while (batch.length);
    }
  };
  if (entries.length) for (const e of entries) await walk(e, "");
  else for (const f of files) out.push({ file: f, rel: f.name });
  return out;
}

const eta = (s: number) => {
  if (!Number.isFinite(s) || s <= 0) return "";
  if (s < 60) return `${Math.ceil(s)} s left`;
  if (s < 3600) return `${Math.ceil(s / 60)} min left`;
  return `${(s / 3600).toFixed(1)} h left`;
};

/** Overall progress of the queue: running count and percentage of bytes. */
export function queueSummary(items: UploadItem[]) {
  const live = items.filter((x) => x.status !== "cancelled");
  const pending = live.filter((x) => x.status === "queued" || active(x.status)).length;
  const failed = live.filter((x) => x.status === "failed").length;
  const total = live.reduce((n, x) => n + x.total, 0);
  const sent = live.reduce((n, x) => n + (x.status === "done" ? x.total : x.sent), 0);
  return { pending, failed, pct: total ? Math.floor((sent / total) * 100) : 100 };
}

/** Upload dialog: a drop zone that starts uploads at once, and the live queue below it. */
export function UploadDialog({
  open,
  onOpenChange,
  dir,
  queue,
  onFiles,
  onFolder,
}: {
  open: boolean;
  onOpenChange: (o: boolean) => void;
  /** Destination folder, shown in the description. */
  dir: string;
  queue: UploadQueue;
  /** Called with picked or dropped files; the caller enqueues them. */
  onFiles: (items: { file: File; rel: string }[]) => void;
  /** Opens the folder picker. */
  onFolder: () => void;
}) {
  const input = useRef<HTMLInputElement>(null);
  const [over, setOver] = useState(false);
  const { items } = queue;
  const sum = queueSummary(items);
  const finished = items.some((x) => x.status === "done" || x.status === "cancelled");
  return (
    <Modal open={open} onOpenChange={onOpenChange} title="Upload" description={`To ${dir}`} width={560}>
      <div className="grid gap-3 text-xs">
        <input
          ref={input}
          type="file"
          multiple
          className="hidden"
          aria-label="Files to upload"
          onChange={(e) => {
            onFiles(Array.from(e.target.files ?? []).map((file) => ({ file, rel: file.name })));
            e.target.value = "";
          }}
        />
        <div
          role="button"
          tabIndex={0}
          aria-label="Drop files here, or press Enter to choose files"
          onClick={() => input.current?.click()}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              input.current?.click();
            }
          }}
          onDragOver={(e) => {
            e.preventDefault();
            setOver(true);
          }}
          onDragLeave={() => setOver(false)}
          onDrop={(e) => {
            e.preventDefault();
            setOver(false);
            void filesFromDrop(e.dataTransfer).then(onFiles);
          }}
          className={`grid cursor-pointer place-items-center gap-1 rounded-lg border border-dashed px-4 py-6 text-center transition outline-none focus-visible:ring-2 focus-visible:ring-accent/50 ${over ? "border-accent bg-accent-soft" : "border-line-strong hover:border-accent hover:bg-panel-2"}`}
        >
          <Upload className="size-4 text-muted" />
          <span className="text-[13px]">
            Drop files or folders here, or <span className="font-medium text-accent underline underline-offset-2">choose files</span>
          </span>
          <span className="text-faint">Uploads start right away. Existing files are never overwritten.</span>
        </div>
        <div className="-mt-1 flex items-center justify-between gap-2">
          <Button variant="ghost" onClick={onFolder}>
            <FolderUp className="size-4" /> Upload a folder…
          </Button>
          {items.length > 0 && (
            <span className="font-mono text-[11px] text-muted tabular-nums">
              {sum.pending ? `${sum.pending} in progress · ${sum.pct}%` : sum.failed ? `${sum.failed} failed` : "All done"}
            </span>
          )}
        </div>

        {items.length > 0 && (
          <ul className="max-h-[40vh] divide-y divide-line overflow-y-auto border-y border-line" aria-label="Uploads" aria-live="polite">
            {items.map((x) => (
              <UploadRow key={x.id} item={x} queue={queue} />
            ))}
          </ul>
        )}

        <div className="flex items-center justify-end gap-2">
          {finished && (
            <Button variant="ghost" onClick={queue.clearFinished}>
              Clear finished
            </Button>
          )}
          <Button onClick={() => onOpenChange(false)}>{queue.busy ? "Hide" : "Close"}</Button>
        </div>
        {queue.busy && <p className="-mt-2 text-right text-faint">Uploads keep running when you hide this dialog.</p>}
      </div>
    </Modal>
  );
}

const LABEL: Record<UploadStatus, string> = {
  queued: "Queued",
  uploading: "Uploading",
  verifying: "Verifying",
  done: "Done",
  failed: "Failed",
  cancelled: "Cancelled",
};

function UploadRow({ item: x, queue }: { item: UploadItem; queue: UploadQueue }) {
  const pct = x.status === "done" ? 100 : x.total ? Math.floor((x.sent / x.total) * 100) : 0;
  const running = active(x.status);
  const tone = x.status === "done" ? "bg-ok" : x.status === "failed" ? "bg-bad" : x.status === "cancelled" ? "bg-line-strong" : "bg-accent";
  const detail =
    x.status === "uploading"
      ? [x.speed > 0 ? bytesRate(x.speed) : "", x.speed > 0 ? eta((x.total - x.sent) / x.speed) : ""].filter(Boolean).join(" · ") || LABEL.uploading
      : x.status === "failed"
        ? (x.error ?? "Failed")
        : LABEL[x.status];
  return (
    <li className="grid gap-1 py-1.5 pr-1">
      <div className="flex min-w-0 items-center gap-2">
        {x.status === "done" ? (
          <CheckCircle2 className="size-3.5 shrink-0 text-ok" aria-label="Done" />
        ) : x.status === "failed" ? (
          <CircleAlert className="size-3.5 shrink-0 text-bad" aria-label="Failed" />
        ) : running ? (
          <Loader2 className="size-3.5 shrink-0 animate-spin text-accent" aria-label={LABEL[x.status]} />
        ) : (
          <span className="size-3.5 shrink-0" />
        )}
        <span className="min-w-0 flex-1 truncate font-mono text-[12px] select-text" title={x.dest}>
          {x.name}
        </span>
        <span className="shrink-0 font-mono text-[11px] text-muted tabular-nums">{bytes(x.total)}</span>
        {x.status === "failed" && (
          <IconButton label={`Retry ${x.name}`} onClick={() => queue.retry(x.id)}>
            <RotateCcw className="size-3.5" />
          </IconButton>
        )}
        <IconButton label={running || x.status === "queued" ? `Cancel ${x.name}` : `Dismiss ${x.name}`} onClick={() => (running || x.status === "queued" ? queue.cancel(x.id) : queue.dismiss(x.id))}>
          <X className="size-3.5" />
        </IconButton>
      </div>
      <div className="flex min-w-0 items-center gap-2 pl-5.5">
        <div className="h-1 w-28 shrink-0 overflow-hidden rounded bg-panel-2" role="progressbar" aria-label={`${x.name} progress`} aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
          <div className={`h-full transition-[width] ${tone}`} style={{ width: `${pct}%` }} />
        </div>
        <span className="w-9 shrink-0 text-right font-mono text-[11px] tabular-nums">{x.status === "queued" ? "—" : `${pct}%`}</span>
        <span className={`min-w-0 flex-1 truncate font-mono text-[11px] tabular-nums ${x.status === "failed" ? "text-bad" : "text-muted"}`} title={detail}>
          {detail}
        </span>
      </div>
    </li>
  );
}
