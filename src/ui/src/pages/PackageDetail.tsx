import { Link, useParams } from "@tanstack/react-router";
import { BookOpen, Download, ExternalLink, FolderTree, Lock, Plug, Server, Settings2, Variable, Workflow } from "lucide-react";
import { type ReactNode, useState } from "react";

import { useCatalog, useInstallations } from "../api/hooks";
import { InstallWizard } from "../components/InstallWizard";
import { PackageList } from "../components/PackageList";
import { ServicePorts } from "../components/tunnels/ServicePorts";
import { Badge, Button, Empty, Mono, PageHeader, Panel, Skeleton, Table, Td } from "../components/ui";

const KIND: Record<string, string> = { native: "Native package", container: "Container", plugin: "Plugin" };

function Card({ icon: Icon, title, children }: { icon: typeof Plug; title: string; children: ReactNode }) {
  return (
    <section className="rounded-lg border border-line bg-panel shadow-[var(--shadow-card)]">
      <header className="card-head flex items-center gap-2 rounded-t-lg border-b border-line px-4 py-2.5">
        <Icon className="size-4 text-accent" />
        <h2 className="text-[13px] font-semibold">{title}</h2>
      </header>
      <div className="p-4 text-[13px]">{children}</div>
    </section>
  );
}

/** One App Store package: what it is, how Forge uses it, its configuration and where it is installed. */
export function PackageDetail() {
  const { packageId } = useParams({ strict: false }) as { packageId: string };
  const catalog = useCatalog();
  const installs = (useInstallations().data ?? []).filter((i) => i.package === packageId);
  const [installing, setInstalling] = useState(false);
  const packages = catalog.data?.packages ?? [];
  const p = packages.find((x) => x.id === packageId);
  if (catalog.isLoading) return <Skeleton />;
  if (!p)
    return (
      <Empty icon={<Plug className="size-5" />} title="Unknown package">
        <Link to="/app-store" className="underline">
          Back to the App Store
        </Link>
      </Empty>
    );
  const i = p.integration;
  const nameOf = (id: string) => packages.find((x) => x.id === id)?.name ?? id;

  return (
    <>
      <PageHeader
        eyebrow="App Store"
        title={p.name}
        description={p.summary}
        actions={
          <>
            <a href={p.homepage} target="_blank" rel="noreferrer">
              <Button variant="ghost">
                <ExternalLink className="size-4" /> Homepage
              </Button>
            </a>
            <Button variant="primary" onClick={() => setInstalling(true)}>
              <Download className="size-4" /> Install
            </Button>
          </>
        }
      />
      <div className="-mt-2 mb-5 flex flex-wrap gap-1.5">
        <Badge tone="accent">{KIND[p.kind] ?? p.kind}</Badge>
        <Badge>{p.category}</Badge>
        <Badge>licence {p.license}</Badge>
        <Badge>{p.versions.length > 1 ? `versions ${p.versions.join(", ")}` : `version ${p.versions[0]}`}</Badge>
        <Badge>{p.os.join(", ")}</Badge>
        {p.ports.length > 0 && <Badge>ports {p.ports.join(", ")}</Badge>}
        {installs.length > 0 && <Badge tone="ok">on {installs.length} node{installs.length === 1 ? "" : "s"}</Badge>}
      </div>

      <div className="grid gap-4 xl:grid-cols-3">
        <div className="space-y-4 xl:col-span-2">
          <Card icon={Workflow} title="How Forge uses it">
            <p>{i?.role ?? p.summary}</p>
            {!!i?.used_by.length && (
              <ul className="mt-3 space-y-1">
                {i.used_by.map((u) => (
                  <li key={u} className="flex gap-2">
                    <span className="mt-1.5 size-1.5 shrink-0 rounded-full bg-accent" /> {u}
                  </li>
                ))}
              </ul>
            )}
            {p.notes && <p className="mt-3 rounded-md bg-panel-2 px-3 py-2 text-xs text-muted">{p.notes}</p>}
          </Card>

          {!!i?.usage.length && (
            <Card icon={BookOpen} title="Using it">
              <ol className="space-y-2">
                {i.usage.map((u, n) => (
                  <li key={n} className="flex gap-3">
                    <span className="grid size-5 shrink-0 place-items-center rounded-full bg-accent-soft text-[11px] font-semibold text-accent">{n + 1}</span>
                    <span>{u}</span>
                  </li>
                ))}
              </ol>
            </Card>
          )}

          <Card icon={Settings2} title="Configuration">
            {p.params.length === 0 ? (
              <p className="text-muted">Nothing to configure: the version is the only choice.</p>
            ) : (
              <Table head={["Setting", "Type", "Default", "What it does"]}>
                {p.params.map((x) => (
                  <tr key={x.key}>
                    <Td>
                      <div className="font-medium">{x.label}</div>
                      <Mono>{x.key}</Mono>
                    </Td>
                    <Td className="text-muted">{x.kind.type === "choice" ? (x.kind as { options: string[] }).options.join(" · ") : x.kind.type}</Td>
                    <Td>
                      <Mono>{x.default || "—"}</Mono>
                    </Td>
                    <Td className="text-muted">
                      {x.help}
                      {x.must_accept && <Badge tone="warn">must accept</Badge>}
                    </Td>
                  </tr>
                ))}
              </Table>
            )}
            {p.requires.length > 0 && (
              <p className="mt-3 text-xs text-muted">
                Needs on the same node first:{" "}
                {p.requires.map((r) => (
                  <Link key={r} to="/app-store/$packageId" params={{ packageId: r }} className="mr-1 text-accent hover:underline">
                    {nameOf(r)}
                  </Link>
                ))}
              </p>
            )}
            <p className="mt-3 text-xs text-muted">Settings are chosen per install. To change them on a node, install again with new values; data is kept.</p>
          </Card>

          <Panel title={`Installed on ${installs.length} node${installs.length === 1 ? "" : "s"}`}>
            {installs.length === 0 ? (
              <Empty icon={<Server className="size-5" />} title="Not installed yet" action={<Button variant="primary" onClick={() => setInstalling(true)}>Install</Button>}>
                Pick the nodes in the installer; each install streams its log live.
              </Empty>
            ) : (
              <PackageList items={installs} />
            )}
          </Panel>
          <ServicePorts kind="package" service={p.id} />
        </div>

        <div className="space-y-4">
          <Card icon={Variable} title="What it gives services">
            {!i?.exports.length ? (
              <p className="text-muted">No variables.</p>
            ) : (
              <>
                <p className="mb-2 text-xs text-muted">
                  Written to <Mono>/etc/forge/pkgs/{p.id}.env</Mono>. Select the package under an app's <b>Package variables</b> to load them.
                </p>
                <ul className="space-y-2">
                  {i.exports.map((x) => (
                    <li key={x.name}>
                      <div className="flex items-center gap-1.5">
                        <Mono>{x.name}</Mono>
                        {x.secret && (
                          <span title="Generated on the node and kept there; never sent to Forge" className="inline-flex items-center gap-0.5 text-[10px] text-warn">
                            <Lock className="size-3" /> secret
                          </span>
                        )}
                      </div>
                      <div className="text-xs text-muted">{x.meaning}</div>
                    </li>
                  ))}
                </ul>
              </>
            )}
          </Card>
          <Card icon={FolderTree} title="On the node">
            {!!i?.services.length && (
              <>
                <div className="mb-1 text-[11px] font-semibold tracking-wide text-faint uppercase">Services</div>
                <div className="mb-3 flex flex-wrap gap-1">
                  {i.services.map((s) => (
                    <Badge key={s}>{s}</Badge>
                  ))}
                </div>
              </>
            )}
            <div className="mb-1 text-[11px] font-semibold tracking-wide text-faint uppercase">Paths</div>
            <ul className="space-y-1.5">
              <li>
                <Mono>/etc/forge/pkgs/{p.id}.env</Mono>
                <div className="text-xs text-muted">variables for services (0600)</div>
              </li>
              {i?.paths.map(([path, what]) => (
                <li key={path}>
                  <Mono>{path}</Mono>
                  <div className="text-xs text-muted">{what}</div>
                </li>
              ))}
            </ul>
          </Card>
        </div>
      </div>

      <InstallWizard pkg={installing ? p : null} catalog={packages} open={installing} onOpenChange={setInstalling} />
    </>
  );
}
