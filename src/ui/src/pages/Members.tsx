import { useMutation } from "@tanstack/react-query";
import { Check, Fingerprint, KeyRound, Loader2, Lock, Plus, ShieldCheck, UserPlus, Users, X } from "lucide-react";
import { Fragment, useMemo, useState } from "react";

import { ApiError, api } from "../api/client";
import { useAllEnvironments, useProjects, useBindings, useClusters, useCreateUser, useGrant, useRevokeBinding, useTenants, useUsers, useWorkspace } from "../api/hooks";
import { loginOf } from "../api/users";
import type { Role, RoleBinding } from "../api/types";
import { errorText, useToast } from "../app/toast";
import { Pager, usePaged } from "../components/Pager";
import { Badge, Button, Confirm, Empty, Field, Input, Modal, PageHeader, Panel, RowMenu, type RowAction, SearchInput, Select, Skeleton, Table, Td, ago, short, useRowMenu } from "../components/ui";

const ROLES: { id: Role; label: string; help: string }[] = [
  { id: "platform_admin", label: "Platform admin", help: "Everything, including plugins and other workspaces." },
  { id: "workspace_admin", label: "Admin", help: "Manage clusters, projects, environments, nodes, members and secrets." },
  { id: "operator", label: "Operator", help: "Add and run nodes: packages, containers, ingress, terminals, jobs." },
  { id: "developer", label: "Developer", help: "View, deploy, run jobs and invoke plugins." },
  { id: "dba", label: "DBA", help: "Database administration and jobs." },
  { id: "auditor", label: "Auditor", help: "Read-only, including the audit log." },
  { id: "customer", label: "Viewer", help: "Read-only." },
];
const roleLabel = (r: string) => ROLES.find((x) => x.id === r)?.label ?? r;

/** Mirrors the Cedar schema (forge.cedarschema) and the MFA guardrail. */
const ACTIONS: { group: string; id: string; label: string; roles: Role[]; mfa?: boolean }[] = [
  { group: "General", id: "view", label: "View resources", roles: ["platform_admin", "workspace_admin", "operator", "developer", "dba", "auditor", "customer"] },
  { group: "General", id: "audit.read", label: "Read the audit log", roles: ["platform_admin", "workspace_admin", "auditor"] },
  { group: "Administration", id: "workspace.manage", label: "Manage clusters, projects, environments, ingress profiles", roles: ["platform_admin", "workspace_admin"] },
  { group: "Administration", id: "user.manage", label: "Manage users and roles", roles: ["platform_admin", "workspace_admin"] },
  { group: "Administration", id: "secret.read", label: "Read vault secrets", roles: ["platform_admin", "workspace_admin"], mfa: true },
  { group: "Administration", id: "plugin.manage", label: "Install and configure plugins", roles: ["platform_admin"], mfa: true },
  { group: "Nodes", id: "node.onboard", label: "Onboard and enroll nodes", roles: ["platform_admin", "workspace_admin", "operator"], mfa: true },
  { group: "Nodes", id: "server.manage", label: "Manage nodes (checks, status)", roles: ["platform_admin", "workspace_admin", "operator"] },
  { group: "Nodes", id: "server.command", label: "Run agent commands", roles: ["platform_admin", "workspace_admin", "operator"], mfa: true },
  { group: "Nodes", id: "process.manage", label: "End processes and change their priority", roles: ["platform_admin", "workspace_admin", "operator"], mfa: true },
  { group: "Nodes", id: "network.tunnel", label: "Open tunnels to services in an environment (forge tunnel)", roles: ["platform_admin", "workspace_admin", "operator", "developer"], mfa: true },
  { group: "Nodes", id: "network.tunnel.any", label: "Open tunnels to raw node ports", roles: ["platform_admin", "workspace_admin"], mfa: true },
  { group: "Nodes", id: "server.rename", label: "Rename nodes (console name and DNS name)", roles: ["platform_admin", "workspace_admin", "operator"] },
  { group: "Alerts", id: "alert.view", label: "See alarms and alert rules", roles: ["platform_admin", "workspace_admin", "operator", "developer", "dba", "auditor"] },
  { group: "Alerts", id: "alert.manage", label: "Manage alert rules and acknowledge alarms", roles: ["platform_admin", "workspace_admin", "operator"] },
  { group: "Alerts", id: "notification.manage", label: "Manage notification channels (where alarm messages are sent)", roles: ["platform_admin", "workspace_admin"], mfa: true },
  { group: "Nodes", id: "terminal.open", label: "Open terminals (SSH)", roles: ["platform_admin", "workspace_admin", "operator"], mfa: true },
  { group: "Nodes", id: "package.manage", label: "Install and remove packages", roles: ["platform_admin", "workspace_admin", "operator"], mfa: true },
  { group: "Nodes", id: "container.manage", label: "Manage containers and overlay networks", roles: ["platform_admin", "workspace_admin", "operator"] },
  { group: "Nodes", id: "proxy.manage", label: "Apply ingress, sites and DNS", roles: ["platform_admin", "workspace_admin", "operator"], mfa: true },
  { group: "Work", id: "job.create", label: "Start jobs", roles: ["platform_admin", "workspace_admin", "operator", "developer", "dba"] },
  { group: "Work", id: "job.cancel", label: "Cancel jobs", roles: ["platform_admin", "workspace_admin", "operator"] },
  { group: "Work", id: "deploy", label: "Deploy services", roles: ["platform_admin", "workspace_admin", "operator", "developer"] },
  { group: "Work", id: "deploy.approve", label: "Approve deployments", roles: ["platform_admin", "workspace_admin", "operator"] },
  { group: "Work", id: "plugin.invoke", label: "Invoke plugins", roles: ["platform_admin", "workspace_admin", "operator", "developer"] },
  { group: "Work", id: "db.admin", label: "Database administration", roles: ["platform_admin", "workspace_admin", "dba"], mfa: true },
];

/** Role binding scopes: platform, workspace, tenant, cluster, project, environment. */
function useScopes() {
  const workspace = useWorkspace().data;
  const tenants = useTenants().data ?? [];
  const clusters = useClusters().data ?? [];
  const groups = useProjects().data ?? [];
  const envs = useAllEnvironments().data ?? [];
  const clusterName = (id: string | null) => (id && clusters.find((c) => c.id === id)?.name) || "Unassigned";
  const groupName = (id: string | null) => (id && groups.find((g) => g.id === id)?.name) || "Unassigned";
  return [
    ...(workspace?.id ? [{ value: `workspace:${workspace.id}`, label: `Workspace · ${workspace.name || "everything"}` }] : []),
    { value: "platform:root", label: "Platform · every workspace" },
    ...tenants.map((t) => ({ value: `tenant:${t.id}`, label: `Tenant · ${t.name}` })),
    ...clusters.map((c) => ({ value: `cluster:${c.id}`, label: `Cluster · ${c.name}` })),
    ...groups.map((g) => ({ value: `project:${g.id}`, label: `Project · ${g.name}` })),
    ...envs.map((e) => ({ value: `environment:${e.id}`, label: `Environment · ${groupName(e.project_id)} / ${e.name} (${clusterName(e.cluster_id)})` })),
  ];
}

type Tab = "access" | "users" | "roles";
export function Members() {
  const users = useUsers();
  const bindings = useBindings();
  const scopes = useScopes();
  // Never default to platform scope: an unloaded workspace must not widen a grant.
  const defaultScope = scopes.find((s) => s.value.startsWith("workspace:"))?.value ?? "";
  const grant = useGrant();
  const createUser = useCreateUser();
  const revoke = useRevokeBinding();
  const toast = useToast();
  const [tab, setTab] = useState<Tab>("access");
  const [inviting, setInviting] = useState(false);
  const [granting, setGranting] = useState(false);
  const [revoking, setRevoking] = useState<RoleBinding>();
  const [clearing, setClearing] = useState<{ id: string; login: string }>();
  const rm = useRowMenu();
  // Recovery for a lost device. Needs an MFA session: a 403 step_up_required
  // opens the step-up dialog (api client) and the request is retried.
  const clearKeys = useMutation({
    mutationFn: (id: string) => api<{ removed?: number }>("DELETE", `/api/v1/users/${encodeURIComponent(id)}/passkeys`),
  });
  const [u, setU] = useState({ username: "", email: "", display_name: "", password: "", method: "password" as "password" | "sso" });
  const [g, setG] = useState({ email: "", role: "operator" as Role, scope: "" });
  const [q, setQ] = useState("");
  const [roleFilter, setRoleFilter] = useState("");
  const [signIn, setSignIn] = useState("");

  const canManageUsers = !(users.error instanceof ApiError);
  const scopeLabel = (b: RoleBinding) =>
    b.scope.kind === "platform" ? "Platform" : (scopes.find((s) => s.value === `${b.scope.kind}:${b.scope.id}`)?.label ?? `${b.scope.kind} · ${short(b.scope.id)}`);
  const byUser = useMemo(() => {
    const m = new Map<string, RoleBinding[]>();
    for (const b of bindings.data ?? []) m.set(b.user_id, [...(m.get(b.user_id) ?? []), b]);
    return m;
  }, [bindings.data]);
  const email = (id: string) => {
    const x = users.data?.find((y) => y.id === id);
    return x ? loginOf(x) : id.slice(0, 8);
  };

  // Users known from the user list, plus any principal seen only in bindings.
  const people = useMemo(() => {
    const list = (users.data ?? []).map((x) => ({ id: x.id, email: loginOf(x), mail: x.email ?? "", name: x.display_name, sso: !!x.oidc_subject, created_at: x.created_at as number | null }));
    for (const id of byUser.keys()) if (!list.some((x) => x.id === id)) list.push({ id, email: id.slice(0, 8), mail: "", name: "", sso: false, created_at: null });
    return list;
  }, [users.data, byUser]);
  const filtered = people.filter(
    (p) =>
      `${p.email} ${p.mail} ${p.name}`.toLowerCase().includes(q.toLowerCase()) &&
      (!signIn || (signIn === "sso") === p.sso) &&
      (!roleFilter || (roleFilter === "none" ? !(byUser.get(p.id)?.length) : byUser.get(p.id)?.some((b) => b.role === roleFilter))),
  );
  const paged = usePaged(filtered, { reset: `${tab}|${q}|${roleFilter}|${signIn}` });
  const shown = paged.rows;

  const filters = (
    <div className="flex flex-wrap items-center gap-2 border-b border-line p-3 sm:flex-nowrap">
      <div className="min-w-[200px] flex-1 [&_input]:w-full [&_label]:w-full">
        <SearchInput value={q} onChange={setQ} placeholder="Search by username, email or name" />
      </div>
      {tab === "access" && (
        <Select className="!w-44 shrink-0" value={roleFilter} onChange={(e) => setRoleFilter(e.target.value)} aria-label="Filter by role">
          <option value="">All roles</option>
          <option value="none">No role</option>
          {ROLES.map((r) => (
            <option key={r.id} value={r.id}>
              {r.label}
            </option>
          ))}
        </Select>
      )}
      <Select className="!w-40 shrink-0" value={signIn} onChange={(e) => setSignIn(e.target.value)} aria-label="Filter by sign-in method">
        <option value="">Any sign-in</option>
        <option value="password">Password</option>
        <option value="sso">SSO</option>
      </Select>
    </div>
  );

  const tabs: [Tab, string, number | null][] = [
    ["access", "Access", people.length],
    ["users", "Users", canManageUsers ? (users.data?.length ?? 0) : null],
    ["roles", "Roles", ROLES.length],
  ];

  return (
    <>
      <PageHeader
        eyebrow="Administration"
        title="Members & roles"
        description="Who can sign in, what each role can do, and which roles every member holds — everywhere, or on one tenant, cluster, project or environment. Changes apply immediately, including to running jobs."
        actions={
          <>
            {canManageUsers && (
              <Button onClick={() => setInviting(true)}>
                <UserPlus className="size-4" /> Add user
              </Button>
            )}
            <Button variant="primary" onClick={() => setGranting(true)}>
              <KeyRound className="size-4" /> Grant role
            </Button>
          </>
        }
      />

      <div className="mb-4 flex gap-1 border-b border-line">
        {tabs.map(([id, label, n]) => (
          <button key={id} onClick={() => setTab(id)} className={`-mb-px inline-flex items-center gap-1.5 border-b-2 px-3 py-2 text-[13px] ${tab === id ? "border-accent text-accent" : "border-transparent text-muted hover:text-fg"}`}>
            {label}
            {n !== null && <span className="rounded bg-panel-2 px-1 text-[11px] text-faint tabular-nums">{n}</span>}
          </button>
        ))}
      </div>

      {tab === "access" && (
        <Panel className="view-enter">
          {filters}
          {bindings.isLoading || users.isLoading ? (
            <Skeleton />
          ) : shown.length === 0 ? (
            <Empty icon={<Users className="size-5" />} title="No members match" />
          ) : (
            <Table head={["Member", "Sign-in", "Roles and scopes", ""]}>
              {shown.map((p) => {
                const actions: RowAction[] = [
                  {
                    label: "Grant role",
                    icon: Plus,
                    onSelect: () => {
                      setG((cur) => ({ ...cur, email: p.email }));
                      setGranting(true);
                    },
                  },
                  ...(byUser.get(p.id) ?? []).map((b) => ({ label: `Revoke ${roleLabel(b.role)} · ${scopeLabel(b)}`, icon: X, danger: true, onSelect: () => setRevoking(b) })),
                  { label: "Remove passkeys", icon: Fingerprint, danger: true, hidden: !canManageUsers, onSelect: () => setClearing({ id: p.id, login: p.email }) },
                ];
                return (
                <tr key={p.id} className="align-top hover:bg-panel-2/60" {...rm.rowProps(actions, `Actions for ${p.email}`)}>
                  <Td>
                    <div className="font-medium">{p.email}</div>
                    {p.name && <div className="text-xs text-faint">{p.name}</div>}
                  </Td>
                  <Td>{p.sso ? <Badge tone="accent">SSO</Badge> : <Badge>Password</Badge>}</Td>
                  <Td>
                    <div className="flex flex-wrap gap-1.5">
                      {(byUser.get(p.id) ?? []).map((b) => (
                        <span key={b.id} className="group inline-flex items-center gap-1 rounded-full border border-line py-0.5 pr-1 pl-2 text-xs" title={`Granted ${ago(b.created_at)}`}>
                          <span className={b.role === "platform_admin" ? "font-medium text-warn" : "font-medium text-accent"}>{roleLabel(b.role)}</span>
                          <span className="text-faint">on {scopeLabel(b)}</span>
                          <button aria-label={`Revoke ${roleLabel(b.role)}`} className="rounded-full p-0.5 text-faint hover:bg-bad-soft hover:text-bad" onClick={() => setRevoking(b)}>
                            <X className="size-3" />
                          </button>
                        </span>
                      ))}
                      {!byUser.get(p.id)?.length && <span className="text-xs text-faint">No roles — can sign in but sees nothing</span>}
                    </div>
                  </Td>
                  <Td className="text-right">
                    <RowMenu actions={actions} label={`Actions for ${p.email}`} />
                  </Td>
                </tr>
                );
              })}
            </Table>
          )}
          {rm.menu}
          <Pager paged={paged} />
        </Panel>
      )}

      {tab === "users" &&
        (canManageUsers ? (
          <Panel className="view-enter">
            {filters}
            {users.isLoading ? (
              <Skeleton />
            ) : (
              <Table head={["Email", "Name", "Sign-in method", "Roles", "Created"]}>
                {shown.map((p) => (
                  <tr key={p.id}>
                    <Td className="font-medium">{p.email}</Td>
                    <Td>{p.name || <span className="text-faint">—</span>}</Td>
                    <Td>{p.sso ? <Badge tone="accent">Single sign-on</Badge> : <Badge>Password</Badge>}</Td>
                    <Td className="text-muted">{byUser.get(p.id)?.length ?? 0}</Td>
                    <Td className="text-muted">{p.created_at ? ago(p.created_at) : "—"}</Td>
                  </tr>
                ))}
              </Table>
            )}
            <Pager paged={paged} />
          </Panel>
        ) : (
          <Empty icon={<Lock className="size-5" />} title="Managing users needs the Platform admin role">
            Creating and listing users needs <span className="font-mono">user.manage</span> on the platform. Ask a platform admin to add the account.
          </Empty>
        ))}

      {tab === "roles" && (
        <div className="view-enter space-y-4">
          <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
            {ROLES.map((r) => {
              const holders = (bindings.data ?? []).filter((b) => b.role === r.id).length;
              return (
                <div key={r.id} className="rounded-lg border border-line bg-panel p-3 shadow-[var(--shadow-card)]">
                  <div className="flex items-center justify-between">
                    <span className="font-semibold">{r.label}</span>
                    <span className="text-xs text-faint">{holders} binding{holders === 1 ? "" : "s"}</span>
                  </div>
                  <p className="mt-1 text-xs text-muted">{r.help}</p>
                  <p className="mt-2 text-xs text-faint">{ACTIONS.filter((a) => a.roles.includes(r.id)).length} of {ACTIONS.length} permissions</p>
                </div>
              );
            })}
          </div>
          <Panel title="What each role can do" description="Enforced by the server's Cedar policies. Actions marked with a shield also need an MFA-verified session.">
            <div className="overflow-x-auto">
              <table className="w-full text-[13px]">
                <thead>
                  <tr className="border-b border-line text-left text-xs text-faint">
                    <th className="px-4 py-2 font-medium">Permission</th>
                    {ROLES.map((r) => (
                      <th key={r.id} className="px-2 py-2 text-center font-medium whitespace-nowrap">
                        {r.label}
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {ACTIONS.map((a, i) => (
                    <Fragment key={a.id}>
                      {a.group !== ACTIONS[i - 1]?.group && (
                        <tr key={`g-${a.group}`}>
                          <td colSpan={ROLES.length + 1} className="bg-panel-2/50 px-4 py-1.5 text-[11px] font-medium tracking-wide text-faint uppercase">
                            {a.group}
                          </td>
                        </tr>
                      )}
                      <tr className="border-b border-line/60 hover:bg-panel-2/40">
                        <td className="px-4 py-2">
                          <div className="flex items-center gap-1.5">
                            {a.label}
                            {a.mfa && <ShieldCheck className="size-3.5 text-accent" aria-label="Needs MFA" />}
                          </div>
                          <div className="font-mono text-[11px] text-faint">{a.id}</div>
                        </td>
                        {ROLES.map((r) => (
                          <td key={r.id} className="px-2 py-2 text-center">
                            {a.roles.includes(r.id) ? <Check className="mx-auto size-4 text-ok" aria-label="Allowed" /> : <span className="text-faint">·</span>}
                          </td>
                        ))}
                      </tr>
                    </Fragment>
                  ))}
                </tbody>
              </table>
            </div>
          </Panel>
        </div>
      )}

      <Modal open={inviting} onOpenChange={setInviting} title="Add user" description="Grant a role right after creating the account.">
        <form
          className="space-y-3"
          onSubmit={(e) => {
            e.preventDefault();
            createUser.mutate(
              { username: u.username || undefined, email: u.email || undefined, display_name: u.display_name, password: u.password },
              {
                onSuccess: (x) => {
                  toast(`User ${loginOf(x)} created`);
                  setInviting(false);
                  setG((cur) => ({ ...cur, email: loginOf(x) }));
                  setU({ username: "", email: "", display_name: "", password: "", method: "password" });
                  setGranting(true);
                },
                onError: (err) =>
                  toast(
                    err instanceof ApiError && err.status === 403 && err.code !== "step_up_required"
                      ? "Only platform admins create users (user.manage on the platform)."
                      : errorText(err),
                    "error",
                  ),
              },
            );
          }}
        >
          <Field label="Username" hint="Letters, digits, '.', '_' or '-'. Optional when an email is given.">
            <Input autoCapitalize="none" spellCheck={false} pattern="[A-Za-z0-9][A-Za-z0-9._\-]{2,63}" required={!u.email} value={u.username} onChange={(e) => setU({ ...u, username: e.target.value })} />
          </Field>
          <Field label="Email" hint="Optional when a username is given.">
            <Input type="email" required={!u.username} value={u.email} onChange={(e) => setU({ ...u, email: e.target.value })} />
          </Field>
          <Field label="Name">
            <Input required value={u.display_name} onChange={(e) => setU({ ...u, display_name: e.target.value })} />
          </Field>
          <p className="rounded-lg border border-line bg-panel-2/40 p-2.5 text-xs text-muted">
            Sign-in method: <b>password</b>. People using single sign-on get an account the first time they sign in with your identity provider; grant their roles afterwards.
          </p>
          {u.method === "password" && (
            <Field label="Initial password" hint="At least 12 characters. Share it securely; the user can enroll MFA after signing in.">
              <Input type="password" required minLength={12} value={u.password} onChange={(e) => setU({ ...u, password: e.target.value })} autoComplete="new-password" />
            </Field>
          )}
          {createUser.error instanceof ApiError && createUser.error.status === 403 && createUser.error.code !== "step_up_required" && (
            <p className="rounded-md border border-bad/40 bg-bad-soft px-2.5 py-2 text-xs text-bad">
              Only platform admins create users. Creating an account needs <span className="font-mono">user.manage</span> on the platform. Ask a platform admin to add the account.
            </p>
          )}
          <div className="flex justify-end">
            <Button variant="primary" disabled={createUser.isPending}>
              {createUser.isPending && <Loader2 className="size-4 animate-spin" />}
              Create user
            </Button>
          </div>
        </form>
      </Modal>

      <Modal open={granting} onOpenChange={setGranting} title="Grant role">
        <form
          className="space-y-3"
          onSubmit={(e) => {
            e.preventDefault();
            grant.mutate(
              { ...g, scope: g.role === "platform_admin" ? "platform:root" : g.scope || defaultScope },
              {
                onSuccess: () => {
                  toast(`${roleLabel(g.role)} granted to ${g.email}`);
                  setGranting(false);
                },
                onError: (err) => toast(errorText(err), "error"),
              },
            );
          }}
        >
          <Field label="Username or email">
            <Input required autoCapitalize="none" spellCheck={false} value={g.email} onChange={(e) => setG({ ...g, email: e.target.value })} list="member-emails" />
            <datalist id="member-emails">
              {people.map((p) => (
                <option key={p.id} value={p.email} />
              ))}
            </datalist>
          </Field>
          <Field label="Role" hint={ROLES.find((r) => r.id === g.role)?.help}>
            <Select value={g.role} onChange={(e) => setG({ ...g, role: e.target.value as Role })}>
              {ROLES.map((r) => (
                <option key={r.id} value={r.id}>
                  {r.label}
                </option>
              ))}
            </Select>
          </Field>
          {g.role !== "platform_admin" && (
            <Field label="Scope">
              <Select required aria-label="Scope" value={g.scope || defaultScope} onChange={(e) => setG({ ...g, scope: e.target.value })}>
                {scopes.map((s) => (
                  <option key={s.value} value={s.value}>
                    {s.label}
                  </option>
                ))}
              </Select>
            </Field>
          )}
          <div className="flex justify-end">
            <Button variant="primary" disabled={grant.isPending || (g.role !== "platform_admin" && !(g.scope || defaultScope))}>
              Grant
            </Button>
          </div>
        </form>
      </Modal>

      <Confirm
        open={!!revoking}
        onOpenChange={(o) => !o && setRevoking(undefined)}
        title="Revoke role?"
        body={revoking ? `${email(revoking.user_id)} loses ${roleLabel(revoking.role)} on ${scopeLabel(revoking)} immediately.` : ""}
        confirmLabel="Revoke"
        busy={revoke.isPending}
        onConfirm={() =>
          revoking &&
          revoke.mutate(revoking.id, {
            onSuccess: () => {
              toast("Role revoked");
              setRevoking(undefined);
            },
            onError: (e) => toast(errorText(e), "error"),
          })
        }
      />

      <Confirm
        open={!!clearing}
        onOpenChange={(o) => !o && !clearKeys.isPending && setClearing(undefined)}
        title="Remove all passkeys?"
        body={
          clearing ? (
            <>
              Every passkey of <strong className="text-fg">{clearing.login}</strong> stops working immediately. They will need their password and authenticator app to
              sign in, and can add new passkeys afterwards. Use this when a device is lost or replaced.
            </>
          ) : (
            ""
          )
        }
        confirmLabel="Remove passkeys"
        busy={clearKeys.isPending}
        onConfirm={() =>
          clearing &&
          clearKeys.mutate(clearing.id, {
            onSuccess: (r) => {
              const n = typeof r?.removed === "number" ? r.removed : undefined;
              toast(n === 0 ? `${clearing.login} had no passkeys` : `Removed ${n ?? "the"} passkey${n === 1 ? "" : "s"} of ${clearing.login}`);
              setClearing(undefined);
            },
            onError: (e) => toast(errorText(e), "error"),
          })
        }
      />
    </>
  );
}
