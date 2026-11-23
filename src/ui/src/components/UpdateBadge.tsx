import { Link } from "@tanstack/react-router";
import { ArrowUpCircle, Loader2 } from "lucide-react";

import { FORGE_BUSY_STATES, useForgeUpdate } from "../api/hooks";

/**
 * Top-right header pill: shown only to administrators who can update, and
 * only when a newer Forge release is out (or an update runs or failed). It
 * opens Administration → Forge updates, where the version is chosen.
 */
export function UpdateBadge() {
  const s = useForgeUpdate().data;
  if (!s || !s.can_update) return null;
  const busy = FORGE_BUSY_STATES.includes(s.state);
  if (!s.available && !busy && s.state !== "failed") return null;

  return (
    <Link
      to="/admin/updates"
      className={`inline-flex max-w-[14rem] items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs font-medium transition ${
        s.state === "failed" ? "border-bad/40 bg-bad-soft text-bad" : "border-accent/40 bg-accent-soft text-accent hover:border-accent"
      }`}
      title={s.latest ? `Forge ${s.latest.version} is available (you run ${s.current || "—"})` : undefined}
    >
      {busy ? <Loader2 className="size-3.5 shrink-0 animate-spin" /> : <ArrowUpCircle className="size-3.5 shrink-0" />}
      <span className="truncate">{busy ? "Updating…" : s.state === "failed" ? "Update failed" : `Update to ${s.latest?.version ?? "new version"}`}</span>
    </Link>
  );
}
