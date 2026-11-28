import { Eraser, RefreshCw } from "lucide-react";
import { useState, type ReactNode } from "react";

import { type DiskBucket, useDockerDiskUsage } from "../../api/dockerResources";
import { Button, Confirm, IconButton, Skeleton, bytes } from "../ui";
import { DockerProblem, type DockerRunner, Spin } from "./shared";

type Prune = "images" | "volumes";

/** One tile: count and bytes, with an optional action. */
function Tile({ label, b, action }: { label: string; b: DiskBucket; action?: ReactNode }) {
  return (
    <div className="flex min-w-0 flex-col gap-1 rounded-md border border-line px-3 py-2.5">
      <div className="text-xs text-muted">{label}</div>
      <div className="font-mono text-lg font-semibold tabular-nums">{b.bytes === undefined ? "—" : bytes(b.bytes)}</div>
      <div className="flex items-center justify-between gap-2">
        <span className="font-mono text-[11px] text-faint tabular-nums">{b.count === undefined ? "—" : `${b.count} item${b.count === 1 ? "" : "s"}`}</span>
        {action}
      </div>
    </div>
  );
}

export function DockerDisk({ server, runner, onOpen }: { server: string; runner: DockerRunner; onOpen: (tab: "containers") => void }) {
  const q = useDockerDiskUsage(server);
  const [prune, setPrune] = useState<Prune | null>(null);
  const close = () => setPrune(null);
  const d = q.data;
  const total = d ? [d.images, d.containers, d.volumes, d.build_cache].reduce((a, b) => a + (b.bytes ?? 0), 0) : undefined;
  const btn = (p: Prune, label: string) => (
    <Button className="!px-2 !py-0.5 text-xs" disabled={runner.pending} onClick={() => setPrune(p)}>
      <Spin on={runner.busy === `${p.slice(0, -1)}_prune:`}>
        <Eraser className="size-3.5" />
      </Spin>{" "}
      {label}
    </Button>
  );

  return (
    <div>
      <div className="flex items-center justify-between gap-2 border-b border-line px-4 py-2">
        <p className="text-xs text-muted">
          Docker's disk use on this node: <span className="font-mono tabular-nums">{total === undefined ? "—" : bytes(total)}</span>
        </p>
        <IconButton label="Refresh disk usage" onClick={() => void q.refetch()} disabled={q.isFetching}>
          <RefreshCw className={`size-4 ${q.isFetching ? "animate-spin" : ""}`} />
        </IconButton>
      </div>
      {q.error ? (
        <DockerProblem error={q.error} server={server} />
      ) : !d ? (
        <Skeleton rows={2} />
      ) : (
        <div className="grid gap-2 p-3 sm:grid-cols-2 xl:grid-cols-4">
          <Tile label="Images" b={d.images} action={btn("images", "Prune dangling")} />
          <Tile
            label="Containers (writable layers)"
            b={d.containers}
            action={
              <Button className="!px-2 !py-0.5 text-xs" onClick={() => onOpen("containers")}>
                Review
              </Button>
            }
          />
          <Tile label="Volumes" b={d.volumes} action={btn("volumes", "Prune anonymous")} />
          <Tile
            label="Build cache"
            b={d.build_cache}
            action={<span className="text-[11px] text-faint" title="Clear it on the node with docker builder prune">no prune here</span>}
          />
        </div>
      )}
      <p className="px-4 pb-3 text-[11px] text-faint">Prunes run as jobs; the numbers refresh when they finish. Networks take no disk; prune them on the Networks tab.</p>

      {prune === "images" && (
        <Confirm
          open
          onOpenChange={(o) => !o && close()}
          title="Prune dangling images?"
          busy={runner.pending}
          body="Docker deletes untagged image layers no container uses. Tagged images stay; to remove unused tagged images too, use Prune on the Images tab. This is host-wide: it covers every unused image on this node, including ones Forge did not create."
          confirmLabel="Prune dangling"
          onConfirm={() => void runner.run({ op: "image_prune", all: false }, "Image prune").then((ok) => ok && close())}
        />
      )}
      {prune === "volumes" && (
        <Confirm
          open
          onOpenChange={(o) => !o && close()}
          title="Prune volumes?"
          busy={runner.pending}
          body="Docker deletes every anonymous volume no container uses, with its data. Named and Forge volumes stay. This is host-wide: it covers every unused anonymous volume on this node, including ones Forge did not create. The data cannot be recovered."
          confirmLabel="Prune volumes"
          onConfirm={() => void runner.run({ op: "volume_prune" }, "Volume prune").then((ok) => ok && close())}
        />
      )}
    </div>
  );
}
