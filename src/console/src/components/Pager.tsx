import { ChevronLeft, ChevronRight } from "lucide-react";
import { useState } from "react";

/** Page sizes offered by `<Pager>`; the first is the default. */
export const PAGE_SIZES = [10, 20, 50] as const;

export interface Paged<T> {
  /** The rows on the current page. */
  rows: T[];
  /** Zero-based page, already clamped to the list. */
  page: number;
  pages: number;
  pageSize: number;
  total: number;
  setPage: (p: number) => void;
  setPageSize: (n: number) => void;
}

/**
 * Client-side paging. `reset` is any value that identifies the current filter
 * (search text, tab, …): when it changes the page returns to 1. The page is
 * clamped during render, so a shrinking list never shows an empty page.
 */
export function usePaged<T>(items: readonly T[] | null | undefined, { reset = "" }: { reset?: string } = {}): Paged<T> {
  const list = Array.isArray(items) ? items : [];
  const [size, setSize] = useState<number>(PAGE_SIZES[0]);
  // The page belongs to the filter it was chosen under; a new filter starts at 0.
  const [at, setAt] = useState<{ reset: string; page: number }>({ reset, page: 0 });
  const total = list.length;
  const pages = Math.max(1, Math.ceil(total / size));
  const page = at.reset === reset ? Math.min(Math.max(0, at.page), pages - 1) : 0;
  return {
    rows: list.slice(page * size, (page + 1) * size),
    page,
    pages,
    pageSize: size,
    total,
    setPage: (p) => setAt({ reset, page: p }),
    setPageSize: (n) => {
      setSize(n);
      setAt({ reset, page: 0 });
    },
  };
}

const fmt = (n: number) => n.toLocaleString();

/** Page-size picker, range and prev/next. Hidden while the list fits on the smallest page. */
export function Pager<T>({ paged, className = "" }: { paged: Paged<T>; className?: string }) {
  const { page, pages, pageSize, total, setPage, setPageSize } = paged;
  if (total <= PAGE_SIZES[0]) return null;
  const from = page * pageSize + 1;
  const to = Math.min(total, (page + 1) * pageSize);
  const btn = "grid size-7 place-items-center rounded-md text-muted hover:bg-panel-2 hover:text-fg disabled:pointer-events-none disabled:opacity-40 focus-visible:outline-2 focus-visible:outline-accent";
  return (
    <div className={`flex flex-wrap items-center justify-between gap-2 border-t border-line px-3 py-1.5 text-xs text-muted ${className}`}>
      <label className="inline-flex items-center gap-1.5">
        Rows
        <select
          className="rounded-md border border-line bg-panel px-1.5 py-0.5 font-mono text-[11px] tabular-nums outline-none focus:border-accent"
          value={pageSize}
          onChange={(e) => setPageSize(Number(e.target.value))}
          aria-label="Rows per page"
        >
          {PAGE_SIZES.map((n) => (
            <option key={n} value={n}>
              {n}
            </option>
          ))}
        </select>
      </label>
      <div className="flex items-center gap-1">
        <span className="font-mono text-[11px] tabular-nums">
          {fmt(from)}–{fmt(to)} of {fmt(total)}
        </span>
        <button type="button" className={btn} disabled={page === 0} onClick={() => setPage(page - 1)} aria-label="Previous page">
          <ChevronLeft className="size-4" />
        </button>
        <button type="button" className={btn} disabled={page >= pages - 1} onClick={() => setPage(page + 1)} aria-label="Next page">
          <ChevronRight className="size-4" />
        </button>
      </div>
    </div>
  );
}
