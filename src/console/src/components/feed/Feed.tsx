// Building blocks of the dense, expandable event feeds (Logs, Audit log):
// the key/value detail grid and the infinite-scroll tail.

import { Loader2, Logs as LogsIcon } from "lucide-react";
import { type ReactNode, useEffect, useRef } from "react";

import { Button, CopyIcon } from "../ui";

/** The vertical key/value grid of an expanded row; put `Meta` items inside. */
export function MetaGrid({ children, className = "" }: { children: ReactNode; className?: string }) {
  return <dl className={`grid max-w-3xl grid-cols-[7.5rem_minmax(0,1fr)] gap-x-3 gap-y-1 text-xs ${className}`}>{children}</dl>;
}

/**
 * One key/value pair per line: the label column, then the value (selectable,
 * optional copy button). `wrap` shows long values in full (commands, paths)
 * instead of truncating them.
 */
export function Meta({ label, value, children, mono, copy, wrap }: { label: string; value?: string; children?: ReactNode; mono?: boolean; copy?: string | null; wrap?: boolean }) {
  return (
    <div className="contents">
      <dt className="truncate text-faint" title={label}>
        {label}
      </dt>
      <dd className={`flex min-w-0 gap-1.5 ${wrap ? "items-start" : "items-center"}`}>
        <span className={`min-w-0 select-text ${wrap ? "whitespace-pre-wrap [overflow-wrap:anywhere]" : "truncate"} ${mono ? "font-mono text-[11px]" : ""}`} title={wrap ? undefined : value}>
          {children ?? value}
        </span>
        {copy && <CopyIcon text={copy} label={`Copy ${label.toLowerCase()}`} className="-my-1" />}
      </dd>
    </div>
  );
}

/** Infinite scroll: a sentinel loads older items when it scrolls into view; the button is the fallback. */
export function FeedTail({
  hasMore,
  loading,
  more,
  error,
  noun = "events",
  end = "End of log",
  children,
}: {
  hasMore: boolean;
  loading: boolean;
  more: () => void;
  error: boolean;
  noun?: string;
  end?: string;
  /** Extra text on the right, such as "200 of 1,204". */
  children?: ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const moreRef = useRef(more);
  moreRef.current = more;
  useEffect(() => {
    const el = ref.current;
    if (!el || !hasMore || error) return;
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((x) => x.isIntersecting)) moreRef.current();
      },
      { rootMargin: "400px" },
    );
    io.observe(el);
    return () => io.disconnect();
  }, [hasMore, error]);
  return (
    <div ref={ref} className="flex items-center justify-center gap-2 border-t border-line py-2 text-xs text-faint">
      {hasMore ? (
        <Button variant="ghost" onClick={more} disabled={loading}>
          {loading && <Loader2 className="size-4 animate-spin" />}
          {error ? `Retry loading older ${noun}` : "Load more"}
        </Button>
      ) : (
        <span className="inline-flex items-center gap-1.5">
          <LogsIcon className="size-3.5" /> {end}
        </span>
      )}
      {children && <span className="font-mono text-[11px] tabular-nums">{children}</span>}
    </div>
  );
}
