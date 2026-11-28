import { Activity, Boxes, Container, Database, HardDrive, Network } from "lucide-react";
import type { ReactNode } from "react";

import type { Server } from "../../api/types";
import type { DockerTab } from "../../app/router";
import { NodeContainers } from "../NodeContainers";
import { DockerApiPanel } from "../node/DockerApiPanel";
import { Panel } from "../ui";
import { DockerDisk } from "./DockerDisk";
import { DockerEvents } from "./DockerEvents";
import { DockerImages } from "./DockerImages";
import { DockerNetworks } from "./DockerNetworks";
import { DockerVolumes } from "./DockerVolumes";
import { useDockerRunner } from "./shared";

const TABS: { id: DockerTab; label: string; icon: typeof Container; hint: string }[] = [
  { id: "containers", label: "Containers", icon: Container, hint: "Containers and stacks" },
  { id: "images", label: "Images", icon: Boxes, hint: "Pulled and built images" },
  { id: "volumes", label: "Volumes", icon: Database, hint: "Named and anonymous volumes" },
  { id: "networks", label: "Networks", icon: Network, hint: "Bridge and overlay networks" },
  { id: "events", label: "Events", icon: Activity, hint: "What the Docker daemon did recently" },
  { id: "disk", label: "Disk", icon: HardDrive, hint: "Space used by images, containers, volumes and build cache" },
];

/** The node's Docker workspace: one sub-tab per kind of Docker object. */
export function DockerWorkspace({ node, tab, onTab }: { node: Server; tab: DockerTab; onTab: (t: DockerTab) => void }) {
  const runner = useDockerRunner(node.id);
  const online = node.status === "online";
  const current = TABS.find((t) => t.id === tab) ?? { id: tab, label: "Docker", icon: Container, hint: "" };

  let body: ReactNode;
  if (tab === "containers") {
    body = (
      <div className="space-y-4">
        <NodeContainers node={node} />
        {node.os === "linux" && <DockerApiPanel node={node} />}
      </div>
    );
  } else if (!online) {
    body = (
      <Panel title={current.label}>
        <p className="px-4 py-6 text-[13px] text-muted">The agent must be online to read Docker {current.label.toLowerCase()}.</p>
      </Panel>
    );
  } else {
    body = (
      <Panel title={current.label} description={current.hint}>
        {tab === "images" && <DockerImages server={node.id} runner={runner} />}
        {tab === "volumes" && <DockerVolumes server={node.id} runner={runner} />}
        {tab === "networks" && <DockerNetworks server={node.id} runner={runner} />}
        {tab === "events" && <DockerEvents server={node.id} />}
        {tab === "disk" && <DockerDisk server={node.id} runner={runner} onOpen={onTab} />}
      </Panel>
    );
  }

  return (
    <div className="space-y-3">
      <div role="tablist" aria-label="Docker sections" className="flex gap-0.5 overflow-x-auto rounded-md bg-panel-2 p-0.5 sm:w-fit">
        {TABS.map((t) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            aria-selected={tab === t.id}
            title={t.hint}
            onClick={() => onTab(t.id)}
            className={`press flex h-7 items-center gap-1.5 rounded px-2.5 text-xs whitespace-nowrap focus-visible:outline-2 focus-visible:outline-accent ${tab === t.id ? "bg-panel font-medium text-fg shadow-sm" : "text-muted hover:text-fg"}`}
          >
            <t.icon className="size-3.5" aria-hidden /> {t.label}
          </button>
        ))}
      </div>
      {body}
      {runner.view}
    </div>
  );
}
