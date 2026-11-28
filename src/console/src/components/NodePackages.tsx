import { useNavigate } from "@tanstack/react-router";
import { Plus } from "lucide-react";

import { useInstallations } from "../api/hooks";
import type { Server } from "../api/types";
import { PackageList } from "./PackageList";
import { Button, Panel } from "./ui";

export function NodePackages({ node }: { node: Server }) {
  const navigate = useNavigate();
  const items = useInstallations(node.id).data ?? [];
  return (
    <Panel
      title="Packages"
      description="App Store software on this node."
      actions={
        <Button onClick={() => void navigate({ to: "/app-store", search: { node: node.id } })}>
          <Plus className="size-4" /> Add package
        </Button>
      }
    >
      {items.length ? <PackageList items={items} showNode={false} /> : <p className="px-4 py-6 text-muted">No packages installed.</p>}
    </Panel>
  );
}
