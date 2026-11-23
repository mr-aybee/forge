import { Link } from "@tanstack/react-router";

import { useTunnels } from "../../api/hooks";
import { CopyValue, Mono, Panel } from "../ui";
import { CannotOpenNote, TunnelTable } from "./parts";

/** Settings → Tunnels: the caller's own open tunnels, and how to open one. */
export function MyTunnels() {
  const q = useTunnels(false);
  const d = q.data;
  return (
    <>
      <Panel title="Your open tunnels" description="Tunnels you opened with the CLI. Closing one ends its connections at once.">
        <TunnelTable
          tunnels={d?.tunnels ?? []}
          loading={q.isLoading}
          supported={d?.supported ?? true}
          error={d?.error ?? null}
          emptyHint={
            <>
              Open one with the CLI, for example <Mono>forge tunnel open --app &lt;name&gt;</Mono>.
            </>
          }
        />
        <CannotOpenNote />
      </Panel>
      <Panel title="Open a tunnel" description="From your computer, signed in with the CLI. The local port is picked once and remembered per target.">
        <dl className="grid grid-cols-[minmax(0,1fr)] gap-y-1.5 px-4 py-3 text-[13px] sm:grid-cols-[140px_minmax(0,1fr)]">
          <dt className="text-muted">Service</dt>
          <dd>
            <CopyValue text="forge tunnel open --app <name>" label="command" />
          </dd>
          <dt className="text-muted">Stack service</dt>
          <dd>
            <CopyValue text="forge tunnel open --stack <stack> --service <service>" label="command" />
          </dd>
          <dt className="text-muted">Database</dt>
          <dd>
            <CopyValue text="forge tunnel open --db <package>" label="command" />
          </dd>
          <dt className="text-muted">SSH</dt>
          <dd>
            <CopyValue text="forge tunnel open --node <node> --ssh" label="command" />
          </dd>
        </dl>
        <p className="border-t border-line px-4 py-2 text-xs text-muted">
          Each service page lists its ports with the exact command. Lifetime is 8 hours unless you pass <Mono>--for</Mono>. Grants and everyone's tunnels are under{" "}
          <Link to="/admin/tunnels" className="text-accent hover:underline">
            Administration → Tunnels
          </Link>
          .
        </p>
      </Panel>
    </>
  );
}
