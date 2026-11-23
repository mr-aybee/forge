import { useNavigate, useSearch } from "@tanstack/react-router";
import { Layers, Plus } from "lucide-react";
import { useState } from "react";

import type { ProjectsSearch } from "../app/router";
import { Button, PageHeader } from "../components/ui";
import { ExplorerDialogs, ProjectExplorer, type ExplorerDialog } from "./explorer/ProjectExplorer";

/**
 * Projects at `/projects`: one explorer of Project → Environment → Service
 * with a details pane. `?sel=` keeps the selection in the URL, so links from
 * elsewhere (`/projects?sel=env:<id>`) open an environment or a project.
 */
export function Projects() {
  const search = useSearch({ from: "/projects" });
  const navigate = useNavigate({ from: "/projects" });
  const [dialog, setDialog] = useState<ExplorerDialog | null>(null);
  const setSearch = (patch: Partial<ProjectsSearch>) => void navigate({ search: (s) => ({ ...s, ...patch }), replace: true });

  return (
    <>
      <PageHeader
        eyebrow="Projects"
        title="Projects"
        description="A project is a product such as shop or billing. Each has environments (test, staging, prod…) that live in a cluster, and services in each environment."
        actions={
          <>
            <Button onClick={() => setDialog({ kind: "new-env", projectId: search.sel?.startsWith("project:") ? search.sel.slice(8) : undefined })}>
              <Layers className="size-4" /> New environment
            </Button>
            <Button variant="primary" onClick={() => setDialog({ kind: "new-project" })}>
              <Plus className="size-4" /> New project
            </Button>
          </>
        }
      />
      <ProjectExplorer search={search} onSearch={setSearch} onDialog={setDialog} />
      {dialog && <ExplorerDialogs dialog={dialog} onClose={() => setDialog(null)} onSelect={(sel) => setSearch({ sel })} />}
    </>
  );
}
