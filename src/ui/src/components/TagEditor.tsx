/**
 * Tags on a node, cluster, environment or service: free-text labels such as
 * `active`, `standby` or `key:value` (`location:karachi`, `dc:fra1`).
 * Shows chips; with `canEdit`, add (Enter or comma) and remove, with
 * suggestions from tags already in use. The server validates and authorizes.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Tag, X } from "lucide-react";
import { useState } from "react";

import { api, get } from "../api/client";
import { errorText, useToast } from "../app/toast";

export type TagKind = "server" | "cluster" | "environment" | "service";

export const useTags = (kind: TagKind, id: string) =>
  useQuery({
    queryKey: ["tags", kind, id],
    queryFn: () => get<{ tags: string[] }>(`/api/v1/tags/${kind}/${encodeURIComponent(id)}`),
    enabled: !!id,
  });

/** Every item's tags of a kind: `{ id: tags }` (for lists). */
export const useTagMap = (kind: TagKind) =>
  useQuery({ queryKey: ["tags", kind], queryFn: () => get<Record<string, string[]>>(`/api/v1/tags/${kind}`) });

const useTagSuggestions = (kind: TagKind, enabled: boolean) =>
  useQuery({
    queryKey: ["tags", kind, "in-use"],
    queryFn: () => get<{ tag: string; count: number }[]>(`/api/v1/tags?kind=${kind}`),
    enabled,
    staleTime: 60_000,
  });

/** Read-only chips (lists, tables). */
export function TagChips({ tags }: { tags: string[] | undefined }) {
  if (!tags?.length) return null;
  return (
    <span className="inline-flex flex-wrap gap-1">
      {tags.map((t) => (
        <span key={t} className="rounded border border-line bg-panel-2 px-1.5 py-px text-[11px] text-muted">
          {t}
        </span>
      ))}
    </span>
  );
}

export function TagEditor({ kind, id, canEdit = true }: { kind: TagKind; id: string; canEdit?: boolean }) {
  const qc = useQueryClient();
  const toast = useToast();
  const tags = useTags(kind, id).data?.tags ?? [];
  const [draft, setDraft] = useState("");
  const [focused, setFocused] = useState(false);
  const suggestions = useTagSuggestions(kind, canEdit && focused).data ?? [];
  const save = useMutation({
    mutationFn: (next: string[]) => api<{ tags: string[] }>("PUT", `/api/v1/tags/${kind}/${encodeURIComponent(id)}`, { tags: next }),
    onSuccess: (r) => {
      qc.setQueryData(["tags", kind, id], r);
      void qc.invalidateQueries({ queryKey: ["tags", kind] });
      void qc.invalidateQueries({ queryKey: ["servers"] });
    },
    onError: (e) => toast(errorText(e), "error"),
  });

  const add = (raw: string) => {
    const parts = raw.split(",").map((s) => s.trim()).filter(Boolean);
    const fresh = parts.filter((p) => !tags.some((t) => t.toLowerCase() === p.toLowerCase()));
    if (fresh.length) save.mutate([...tags, ...fresh]);
    setDraft("");
  };
  const remove = (t: string) => save.mutate(tags.filter((x) => x !== t));
  const matches = draft
    ? suggestions.filter((s) => s.tag.toLowerCase().includes(draft.toLowerCase()) && !tags.includes(s.tag)).slice(0, 6)
    : [];

  return (
    <div className="flex flex-wrap items-center gap-1.5">
      <Tag className="size-3.5 text-muted" aria-hidden />
      {tags.map((t) => (
        <span key={t} className="inline-flex items-center gap-1 rounded border border-line bg-panel-2 px-1.5 py-0.5 text-xs">
          {t}
          {canEdit && (
            <button type="button" aria-label={`Remove tag ${t}`} className="text-muted hover:text-bad" onClick={() => remove(t)} disabled={save.isPending}>
              <X className="size-3" />
            </button>
          )}
        </span>
      ))}
      {!tags.length && !canEdit && <span className="text-xs text-muted">No tags</span>}
      {canEdit && (
        <span className="relative">
          <input
            value={draft}
            placeholder={tags.length ? "Add tag" : "Add a tag, e.g. dc:fra1"}
            aria-label="Add tag"
            maxLength={64}
            className="w-40 rounded border border-transparent bg-transparent px-1 py-0.5 text-xs outline-none focus:border-line"
            onFocus={() => setFocused(true)}
            onBlur={() => setTimeout(() => setFocused(false), 150)}
            onChange={(e) => {
              const v = e.target.value;
              if (v.endsWith(",")) add(v);
              else setDraft(v);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                if (draft.trim()) add(draft);
              } else if (e.key === "Backspace" && !draft && tags.length) {
                remove(tags[tags.length - 1]!);
              }
            }}
          />
          {focused && matches.length > 0 && (
            <ul className="absolute left-0 top-full z-20 mt-1 min-w-40 rounded-md border border-line bg-panel py-1 text-xs shadow-lg">
              {matches.map((s) => (
                <li key={s.tag}>
                  <button type="button" className="flex w-full justify-between gap-3 px-2 py-1 text-left hover:bg-panel-2" onMouseDown={() => add(s.tag)}>
                    <span>{s.tag}</span>
                    <span className="text-muted">{s.count}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </span>
      )}
    </div>
  );
}
