import { useInfiniteQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronRight, Download, File as FileIcon, Folder, Loader2, RefreshCw, Trash2, Upload, X } from "lucide-react";
import { useRef, useState } from "react";

import { MAX_UPLOAD, fetchObjects, objectUrl, objectsKey, uploadObject, useDeleteObject } from "../../api/storage";
import { errorText, useToast } from "../../app/toast";
import { Button, Confirm, Empty, ErrorNote, IconButton, Input, Skeleton, Table, Td, ago, bytes, time, toMs } from "../ui";

interface Transfer {
  id: number;
  key: string;
  sent: number;
  total: number;
  error?: string;
  done?: boolean;
  abort: AbortController;
}

let seq = 0;

/**
 * Browse one bucket by `/`-delimited prefix; upload, download and delete
 * objects. With `readOnly` (a read-only bucket grant) the upload and delete
 * controls are left out: the server would refuse them.
 */
export function ObjectExplorer({ storeId, bucket, readOnly = false }: { storeId: string; bucket: string; readOnly?: boolean }) {
  const [prefix, setPrefix] = useState("");
  const [filter, setFilter] = useState("");
  const [del, setDel] = useState<string | null>(null);
  const [transfers, setTransfers] = useState<Transfer[]>([]);
  const fileRef = useRef<HTMLInputElement>(null);
  const qc = useQueryClient();
  const toast = useToast();
  const remove = useDeleteObject();

  const listPrefix = prefix + filter;
  const list = useInfiniteQuery({
    queryKey: objectsKey(storeId, bucket, listPrefix),
    queryFn: ({ pageParam }) => fetchObjects(storeId, bucket, listPrefix, pageParam),
    initialPageParam: null as string | null,
    getNextPageParam: (p) => p.next_token ?? undefined,
    retry: false,
    staleTime: 10_000,
  });
  const pages = list.data?.pages ?? [];
  const prefixes = pages.flatMap((p) => p.prefixes ?? []);
  const objects = pages.flatMap((p) => p.objects ?? []).filter((o) => o?.key && o.key !== prefix);

  const crumbs = prefix.split("/").filter(Boolean);
  const go = (p: string) => {
    setPrefix(p);
    setFilter("");
  };
  const rel = (k: string) => (k.startsWith(prefix) ? k.slice(prefix.length) : k);
  const refresh = () => qc.invalidateQueries({ queryKey: ["objects", storeId, bucket] });

  const patch = (id: number, p: Partial<Transfer>) => setTransfers((ts) => ts.map((t) => (t.id === id ? { ...t, ...p } : t)));

  const upload = async (files: FileList | null) => {
    for (const file of Array.from(files ?? [])) {
      const key = prefix + file.name;
      if (file.size > MAX_UPLOAD) {
        toast(`${file.name} is ${bytes(file.size)}; uploads are limited to 5 GiB`, "error");
        continue;
      }
      const t: Transfer = { id: ++seq, key, sent: 0, total: file.size, abort: new AbortController() };
      setTransfers((ts) => [...ts, t]);
      try {
        await uploadObject(storeId, bucket, key, file, (sent, total) => patch(t.id, { sent, total }), t.abort.signal);
        patch(t.id, { done: true, sent: file.size });
        void refresh();
        setTimeout(() => setTransfers((ts) => ts.filter((x) => x.id !== t.id)), 2500);
      } catch (e) {
        patch(t.id, { error: errorText(e) });
      }
    }
    if (fileRef.current) fileRef.current.value = "";
  };

  return (
    <div className="min-w-0">
      <div className="flex flex-wrap items-center gap-2 border-b border-line px-3 py-2">
        <nav aria-label="Path" className="flex min-w-0 flex-1 flex-wrap items-center gap-0.5 font-mono text-xs">
          <button className="rounded px-1 hover:bg-panel-2 hover:text-accent" onClick={() => go("")} title={`${bucket} (root)`}>
            {bucket}
          </button>
          {crumbs.map((c, i) => (
            <span key={i} className="flex min-w-0 items-center gap-0.5">
              <ChevronRight className="size-3 shrink-0 text-faint" />
              <button className="max-w-[12rem] truncate rounded px-1 hover:bg-panel-2 hover:text-accent" title={c} onClick={() => go(crumbs.slice(0, i + 1).join("/") + "/")}>
                {c}
              </button>
            </span>
          ))}
        </nav>
        <Input className="!w-44 font-mono !text-xs" placeholder="Filter by key prefix…" aria-label="Filter by key prefix" value={filter} onChange={(e) => setFilter(e.target.value)} />
        <IconButton label="Refresh" onClick={() => void refresh()}>
          <RefreshCw className={`size-4 ${list.isFetching ? "animate-spin" : ""}`} />
        </IconButton>
        {readOnly ? (
          <span className="text-xs text-muted" title="Your grant for this bucket lets you list and download objects">
            Read only
          </span>
        ) : (
          <>
            <input ref={fileRef} type="file" multiple className="hidden" aria-label="Files to upload" onChange={(e) => void upload(e.target.files)} />
            <Button onClick={() => fileRef.current?.click()} title="Up to 5 GiB per file">
              <Upload className="size-4" /> Upload
            </Button>
          </>
        )}
      </div>

      {transfers.length > 0 && (
        <ul className="divide-y divide-line border-b border-line text-xs">
          {transfers.map((t) => {
            const pct = t.total ? Math.round((t.sent / t.total) * 100) : 0;
            return (
              <li key={t.id} className="flex items-center gap-3 px-3 py-1.5">
                <span className="min-w-0 flex-1 truncate font-mono" title={t.key}>
                  {t.key}
                </span>
                {t.error ? (
                  <span className="max-w-[50%] truncate text-bad" title={t.error}>
                    {t.error}
                  </span>
                ) : (
                  <>
                    <div className="h-1 w-32 overflow-hidden rounded bg-panel-2">
                      <div className={`h-full ${t.done ? "bg-ok" : "bg-accent"}`} style={{ width: `${pct}%` }} />
                    </div>
                    <span className="w-28 text-right font-mono text-[11px] tabular-nums">
                      {t.done ? "done" : `${bytes(t.sent)} / ${bytes(t.total)}`}
                    </span>
                  </>
                )}
                <IconButton
                  label={t.error || t.done ? "Dismiss" : "Cancel upload"}
                  className="!size-6"
                  onClick={() => (t.error || t.done ? setTransfers((ts) => ts.filter((x) => x.id !== t.id)) : t.abort.abort())}
                >
                  <X className="size-3.5" />
                </IconButton>
              </li>
            );
          })}
        </ul>
      )}

      {list.isLoading ? (
        <Skeleton rows={4} />
      ) : list.error ? (
        <ErrorNote error={list.error} />
      ) : !prefixes.length && !objects.length ? (
        <Empty icon={<Folder className="size-5" />} title={filter ? "No keys match" : prefix ? "This folder is empty" : "This bucket is empty"} action={readOnly ? undefined : <Button onClick={() => fileRef.current?.click()}><Upload className="size-4" /> Upload files</Button>}>
          {filter ? `Nothing starts with “${listPrefix}”.` : readOnly ? "Nothing has been uploaded here yet." : "Upload files here; folders appear from key prefixes."}
        </Empty>
      ) : (
        <Table head={["Key", "Size", "Modified", ""]}>
          {prefixes.map((p) => (
            <tr key={`p:${p}`} className="text-[13px] hover:bg-panel-2/50">
              <Td className="max-w-0 w-full py-1.5">
                <button className="flex max-w-full items-center gap-1.5 hover:text-accent" onClick={() => go(p)} title={p}>
                  <Folder className="size-3.5 shrink-0 text-accent" />
                  <span className="truncate font-mono text-xs">{rel(p)}</span>
                </button>
              </Td>
              <Td className="py-1.5 text-xs text-faint">—</Td>
              <Td className="py-1.5 text-xs text-faint">—</Td>
              <Td className="py-1.5" />
            </tr>
          ))}
          {objects.map((o) => {
            const ms = toMs(o.last_modified);
            return (
              <tr key={`o:${o.key}`} className="text-[13px] hover:bg-panel-2/50">
                <Td className="max-w-0 w-full py-1.5">
                  <span className="flex items-center gap-1.5" title={o.key}>
                    <FileIcon className="size-3.5 shrink-0 text-faint" />
                    <span className="truncate font-mono text-xs select-all">{rel(o.key)}</span>
                  </span>
                </Td>
                <Td className="py-1.5 text-right font-mono text-[11px] whitespace-nowrap tabular-nums">{bytes(o.size)}</Td>
                <Td className="py-1.5 text-xs whitespace-nowrap text-muted" title={ms ? time(ms) : undefined}>
                  {ms ? ago(ms) : "—"}
                </Td>
                <Td className="py-1.5 whitespace-nowrap">
                  <span className="flex justify-end gap-0.5">
                    <a
                      href={objectUrl(storeId, bucket, o.key)}
                      download
                      aria-label={`Download ${o.key}`}
                      title="Download"
                      className="grid size-7 place-items-center rounded-md text-muted hover:bg-panel-2 hover:text-fg"
                    >
                      <Download className="size-3.5" />
                    </a>
                    {!readOnly && (
                      <IconButton label={`Delete ${o.key}`} className="!size-7 hover:!text-bad" onClick={() => setDel(o.key)}>
                        <Trash2 className="size-3.5" />
                      </IconButton>
                    )}
                  </span>
                </Td>
              </tr>
            );
          })}
        </Table>
      )}
      {list.hasNextPage && (
        <div className="flex justify-center border-t border-line p-2">
          <Button variant="ghost" onClick={() => void list.fetchNextPage()} disabled={list.isFetchingNextPage}>
            {list.isFetchingNextPage && <Loader2 className="size-4 animate-spin" />}
            Load more
          </Button>
        </div>
      )}

      <Confirm
        open={!!del}
        onOpenChange={(o) => !o && setDel(null)}
        title="Delete object?"
        body={
          <>
            <span className="font-mono text-xs break-all text-fg">
              {bucket}/{del}
            </span>{" "}
            is deleted permanently. Versioning is not managed by Forge, so there is no undo here.
          </>
        }
        confirmLabel="Delete object"
        busy={remove.isPending}
        onConfirm={() =>
          del &&
          remove.mutate(
            { id: storeId, bucket, key: del },
            {
              onSuccess: () => {
                toast(`Deleted ${del}`);
                setDel(null);
              },
              onError: (e) => toast(errorText(e), "error"),
            },
          )
        }
      />
    </div>
  );
}
