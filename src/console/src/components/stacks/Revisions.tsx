import { History, RotateCcw } from "lucide-react";
import { useMemo, useState } from "react";

import { type StackDetail, useRevision, useRevisions } from "../../api/stacks";
import { Badge, Button, CopyButton, ErrorNote, Skeleton, ago, time } from "../ui";
import { CodeDiff } from "../CodeField";
import { diffLines } from "./diff";
import { YamlEditor } from "./parts";

/** Compose YAML before/after in Monaco's diff view, with +/- counts; unchanged runs fold. */
export function DiffView({ before, after, height = 380 }: { before: string; after: string; height?: number }) {
  const counts = useMemo(() => {
    const list = diffLines(before, after);
    return { added: list.filter((l) => l.op === "add").length, removed: list.filter((l) => l.op === "del").length };
  }, [before, after]);
  if (!counts.added && !counts.removed) return <p className="rounded-md border border-line px-3 py-4 text-center text-xs text-muted">No differences.</p>;
  return (
    <div className="space-y-1">
      <div className="font-mono text-[11px] tabular-nums">
        <span className="text-ok">+{counts.added}</span> <span className="text-bad">-{counts.removed}</span>
      </div>
      <CodeDiff before={before} after={after} lang="yaml" height={height} aria-label="Changes" />
    </div>
  );
}

/** Revisions: list, view one, diff it against the latest, roll back to it. */
export function RevisionsPanel({ d, onRollback }: { d: StackDetail; onRollback: (rev: number) => void }) {
  const revs = useRevisions(d.stack.id);
  const list = revs.data ?? [];
  const [sel, setSel] = useState<number>();
  const [mode, setMode] = useState<"diff" | "file">("diff");
  const picked = sel ?? list.find((r) => r.revision !== d.stack.revision)?.revision ?? list[0]?.revision;
  const rev = useRevision(d.stack.id, picked);
  const content = rev.data?.content ?? "";

  if (revs.isLoading) return <Skeleton rows={3} />;
  if (revs.error) return <ErrorNote error={revs.error} />;
  if (!list.length) return <p className="px-4 py-6 text-muted">No revisions recorded.</p>;
  return (
    <div className="grid min-w-0 gap-0 md:grid-cols-[260px_minmax(0,1fr)]">
      <ul className="max-h-[60vh] divide-y divide-line overflow-y-auto border-b border-line md:border-r md:border-b-0" aria-label="Revisions">
        {list.map((r) => {
          const active = r.revision === picked;
          return (
            <li key={r.revision}>
              <button onClick={() => setSel(r.revision)} aria-current={active ? "true" : undefined} className={`block w-full min-w-0 px-3 py-1.5 text-left ${active ? "bg-accent-soft" : "hover:bg-panel-2/60"}`}>
                <div className="flex items-center gap-1.5">
                  <span className="font-mono text-xs font-medium tabular-nums">r{r.revision}</span>
                  {r.revision === d.stack.revision && <Badge tone="accent">latest</Badge>}
                  {r.revision === d.stack.deployed_revision && <Badge tone="ok">deployed</Badge>}
                  <span className="ml-auto text-[11px] text-faint" title={time(r.created_at)}>
                    {ago(r.created_at)}
                  </span>
                </div>
                <div className="truncate text-[11px] text-muted" title={r.note ?? undefined}>
                  {r.note ?? <span className="text-faint">no note</span>}
                </div>
                <div className="truncate font-mono text-[10.5px] text-faint" title={`sha256 ${r.sha256}`}>
                  {r.author || "—"} · {r.sha256.slice(0, 12) || "—"}
                </div>
              </button>
            </li>
          );
        })}
      </ul>
      <div className="min-w-0 space-y-2 p-3">
        <div className="flex flex-wrap items-center gap-2">
          <History className="size-3.5 text-muted" />
          <span className="font-mono text-xs">r{picked ?? "—"}</span>
          <div className="flex rounded-md border border-line p-0.5 text-xs">
            {(["diff", "file"] as const).map((m) => (
              <button key={m} onClick={() => setMode(m)} className={`rounded px-2 py-0.5 ${mode === m ? "bg-accent-soft text-accent" : "text-muted"}`}>
                {m === "diff" ? `Changes → r${d.stack.revision}` : "File"}
              </button>
            ))}
          </div>
          <span className="flex-1" />
          {content && <CopyButton text={content} />}
          {picked !== undefined && (
            <span title={picked === d.stack.deployed_revision ? "This revision is already deployed" : undefined} className="inline-flex">
              <Button onClick={() => onRollback(picked)} disabled={picked === d.stack.deployed_revision}>
                <RotateCcw className="size-4" /> {picked === d.stack.revision ? "Deploy this" : "Roll back to this"}
              </Button>
            </span>
          )}
        </div>
        {rev.isLoading ? (
          <Skeleton rows={4} />
        ) : rev.error ? (
          <ErrorNote error={rev.error} />
        ) : mode === "diff" ? (
          picked === d.stack.revision ? <p className="rounded-md border border-line px-3 py-4 text-center text-xs text-muted">This is the latest revision. Pick an older one to compare.</p> : <DiffView before={content} after={d.content} height={460} />
        ) : (
          <YamlEditor value={content} docKey={`rev-${d.stack.id}-${picked}-${content.length}`} onChange={() => {}} readOnly height="h-[52vh]" />
        )}
      </div>
    </div>
  );
}
