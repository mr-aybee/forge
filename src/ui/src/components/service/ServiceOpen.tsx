import { ExternalLink, SquareTerminal } from "lucide-react";

import { useTerminals } from "../../app/terminals";
import { nodeName, type Server, type Service } from "../../api/types";
import { Button, MenuButton } from "../ui";

/** Opens a console URL in a new tab, without giving the page a handle on Forge's window. */
const openUrl = (url: string) => void window.open(url, "_blank", "noopener,noreferrer");

/**
 * **Open console** and **Open shell** on the service page. The server
 * resolves both (`console_urls`, `shell_targets`) and lists only the nodes
 * the caller may open a shell on; the shell endpoints still authorize and
 * audit every session. One target is a button, several a menu per node.
 */
export function ServiceOpen({ app, nodes }: { app: Service; nodes: Server[] }) {
  const terminals = useTerminals();
  const urls = app.console_urls ?? [];
  const shells = app.shell_targets ?? [];
  const nodeOf = (id: string) => nodes.find((n) => n.id === id);
  const label = (id: string | null | undefined, fallback: string) => {
    const n = id ? nodeOf(id) : undefined;
    return n ? nodeName(n) : fallback;
  };

  const openShell = (serverId: string, container: string | null | undefined) => {
    const n = nodeOf(serverId);
    if (!n) return;
    if (container) terminals.openContainer(n, { id: container, name: container });
    else terminals.open(n);
  };
  const shellHint = (container: string | null | undefined) => (container ? `docker exec into ${container}` : "SSH terminal on the node");

  return (
    <>
      {urls.length === 1 && urls[0] ? (
        <a href={urls[0].url} target="_blank" rel="noopener noreferrer" title={urls[0].url} className="inline-flex items-center gap-1.5 rounded-md border border-line bg-panel px-3 py-1.5 text-[13px] font-medium hover:border-accent/60 hover:text-accent focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-accent">
          <ExternalLink className="size-4" /> Open console
        </a>
      ) : urls.length > 1 ? (
        <MenuButton title="Open the service's web console in a new tab" actions={urls.map((u) => ({ label: `${label(u.server_id, u.hostname)} · ${u.url}`, icon: ExternalLink, onSelect: () => openUrl(u.url) }))}>
          <ExternalLink className="size-4" /> Open console
        </MenuButton>
      ) : null}
      {shells.length === 1 && shells[0] ? (
        <Button title={shellHint(shells[0].container)} disabled={!nodeOf(shells[0].server_id)} onClick={() => shells[0] && openShell(shells[0].server_id, shells[0].container)}>
          <SquareTerminal className="size-4" /> Open shell
        </Button>
      ) : shells.length > 1 ? (
        <MenuButton title="Open a shell on one of its nodes" actions={shells.map((t) => ({ label: label(t.server_id, t.server_id.slice(0, 8)), icon: SquareTerminal, disabled: !nodeOf(t.server_id), onSelect: () => openShell(t.server_id, t.container) }))}>
          <SquareTerminal className="size-4" /> Open shell
        </MenuButton>
      ) : null}
    </>
  );
}
