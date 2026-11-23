import { useQuery } from "@tanstack/react-query";
import { Check, Copy } from "lucide-react";
import { useState } from "react";

import { get } from "../api/client";
import type { FabricResponse } from "./fabric/FabricForm";
import { useToast } from "../app/toast";
import { COPY_BLOCKED, copyText } from "../lib/clipboard";

/** A cluster's network plan (cached; shared by node, cluster and app pages). */
export const useClusterFabric = (clusterId?: string | null) =>
  useQuery({
    queryKey: ["fabric", clusterId],
    queryFn: () => get<FabricResponse>(`/api/v1/clusters/${clusterId}/fabric`),
    enabled: !!clusterId,
    staleTime: 30_000,
  });

/** Address of an owner in a fabric plan, without a /32 suffix. */
export function leaseOf(f: FabricResponse | undefined, kind: "node" | "block" | "service", owner: string) {
  const l = f?.leases?.find((x) => x.kind === kind && x.owner === owner);
  return l ? l.cidr.replace(/\/32$/, "") : undefined;
}

/** A small "label value" chip; click copies the value. */
export function IpTag({ label, value, title }: { label: string; value: string; title?: string }) {
  const [copied, setCopied] = useState(false);
  const toast = useToast();
  return (
    <button
      type="button"
      title={title ?? `Copy ${value}`}
      onClick={() =>
        void copyText(value).then((ok) => {
          if (!ok) return toast(COPY_BLOCKED, "error");
          setCopied(true);
          setTimeout(() => setCopied(false), 1200);
        })
      }
      className="group inline-flex items-center overflow-hidden rounded-md border border-line bg-panel text-xs shadow-sm transition hover:border-accent"
    >
      <span className="bg-panel-2 px-2 py-1 text-[10px] font-semibold tracking-wider text-muted uppercase">{label}</span>
      <span className="px-2 py-1 font-mono">{value}</span>
      <span className="pr-2 text-faint group-hover:text-accent">{copied ? <Check className="size-3.5 text-ok" /> : <Copy className="size-3.5" />}</span>
    </button>
  );
}

export function IpTags({ tags }: { tags: { label: string; value?: string | null; title?: string }[] }) {
  const shown = tags.filter((t): t is { label: string; value: string; title?: string } => !!t.value);
  if (!shown.length) return null;
  return (
    <div className="-mt-2 mb-4 flex flex-wrap gap-2">
      {shown.map((t) => (
        <IpTag key={t.label} {...t} />
      ))}
    </div>
  );
}
