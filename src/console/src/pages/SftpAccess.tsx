import { useQuery } from "@tanstack/react-query";
import { FolderLock } from "lucide-react";
import { useState } from "react";

import { listAllGrants } from "../api/files";
import { GrantList } from "../components/files/GrantList";
import { Empty, ErrorNote, PageHeader, SearchInput, Select, Skeleton } from "../components/ui";

/** Every temporary SFTP access the caller may see, across nodes. */
export function SftpAccess() {
  const [q, setQ] = useState("");
  const [status, setStatus] = useState("");
  const [server, setServer] = useState("");
  const grants = useQuery({ queryKey: ["all-file-grants", status], queryFn: () => listAllGrants({ status }), retry: false, refetchInterval: 30_000 });
  const all = grants.data ?? [];
  const nodes = [...new Map(all.map((g) => [g.server_id, g.server_name ?? g.server_id])).entries()].sort((a, b) => a[1].localeCompare(b[1]));
  const needle = q.trim().toLowerCase();
  const shown = all.filter(
    (g) =>
      (!server || g.server_id === server) &&
      (!needle || [g.name, g.id, g.created_by, g.server_name ?? ""].some((f) => f.toLowerCase().includes(needle))),
  );

  return (
    <div className="grid gap-4">
      <PageHeader
        title="SFTP access"
        description="Temporary SFTP access on every node you may see. Edit folders, permissions, restrictions and expiry, reset passwords or revoke access. Issue new access from a node's Files tab."
      />
      <div className="flex flex-wrap items-center gap-2">
        <div className="w-64">
          <SearchInput value={q} onChange={setQ} placeholder="Search name, id, creator, node…" />
        </div>
        <div className="w-48">
          <Select value={server} onChange={(e) => setServer(e.target.value)} aria-label="Node">
            <option value="">All nodes</option>
            {nodes.map(([id, name]) => (
              <option key={id} value={id}>
                {name}
              </option>
            ))}
          </Select>
        </div>
        <div className="w-40">
          <Select value={status} onChange={(e) => setStatus(e.target.value)} aria-label="Status">
            <option value="">Any status</option>
            <option value="active">Active</option>
            <option value="expired">Expired</option>
            <option value="revoked">Revoked</option>
          </Select>
        </div>
      </div>
      {grants.isLoading ? (
        <Skeleton rows={5} />
      ) : grants.error ? (
        <ErrorNote error={grants.error} />
      ) : !shown.length ? (
        <Empty icon={<FolderLock className="size-5" />} title={all.length ? "Nothing matches" : "No SFTP access"}>
          {all.length ? "Change the search or filters." : "Issue temporary access from a node's Files tab with SFTP access."}
        </Empty>
      ) : (
        <GrantList grants={shown} showNode onChanged={() => void grants.refetch()} />
      )}
    </div>
  );
}
