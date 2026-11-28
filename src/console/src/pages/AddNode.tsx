import * as Tabs from "@radix-ui/react-tabs";
import { Link } from "@tanstack/react-router";
import {
  ArrowRight,
  Fingerprint,
  KeyRound,
  Lock,
  ShieldCheck,
  Terminal,
  Wand2,
} from "lucide-react";
import { useEffect, useState } from "react";

import {
  useAllEnvironments,
  useClusters,
  useCreateEnrollmentToken,
  useJob,
  useMe,
  useOnboardNode,
  useOnboardingConsole,
  useOnboardingInfo,
  useProbeNode,
  type EnrollmentToken,
} from "../api/hooks";
import type { HostKey } from "../api/types";
import { errorText, useToast } from "../app/toast";
import { TokenConsole } from "../components/Console";
import { JobDetail } from "../components/JobsDrawer";
import {
  Button,
  CopyButton,
  Field,
  Input,
  PageHeader,
  Panel,
  Select,
  Switch,
  TextArea,
} from "../components/ui";

/** The terminal account the installer creates (D-078). */
interface TerminalAccount {
  create: boolean;
  name: string;
  sudo: boolean;
}
const DEFAULT_ACCOUNT: TerminalAccount = {
  create: true,
  name: "forge-ops",
  sudo: false,
};
const ACCOUNT_NAME = /^[a-z_][a-z0-9_-]{0,31}$/;

/** Name of the account to create, or `undefined` when none is wanted. */
const accountName = (a: TerminalAccount) =>
  a.create && a.name.trim() ? a.name.trim() : undefined;

function TerminalAccountFields({
  value,
  onChange,
}: {
  value: TerminalAccount;
  onChange: (v: TerminalAccount) => void;
}) {
  const invalid = value.create && !ACCOUNT_NAME.test(value.name.trim());
  return (
    <div className="space-y-2">
      <Switch
        checked={value.create}
        onChange={(create) => onChange({ ...value, create })}
        label="Create a terminal account"
      />
      {value.create && (
        <div className="flex flex-wrap items-end gap-3 pl-9">
          <div className="w-56">
            <Field
              label="Account name"
              hint={
                invalid
                  ? "Use a-z, 0-9, _ and -, starting with a letter."
                  : undefined
              }
            >
              <Input
                value={value.name}
                maxLength={32}
                onChange={(e) => onChange({ ...value, name: e.target.value })}
                spellCheck={false}
                autoComplete="off"
              />
            </Field>
          </div>
          <div className="pb-2">
            <Switch
              checked={value.sudo}
              onChange={(sudo) => onChange({ ...value, sudo })}
              label="Allow sudo"
            />
          </div>
        </div>
      )}
      <p className="text-xs text-faint">
        No password: it signs in only through Forge's browser terminal, with
        short-lived certificates for this node. Sudo is passwordless and off
        unless you allow it.
      </p>
    </div>
  );
}

function Placement({
  cluster,
  env,
  onCluster,
  onEnv,
}: {
  cluster: string;
  env: string;
  onCluster: (v: string) => void;
  onEnv: (v: string) => void;
}) {
  const clusters = useClusters().data ?? [];
  const envs = useAllEnvironments().data ?? [];
  return (
    <div className="grid gap-3 sm:grid-cols-2">
      <Field label="Cluster">
        <Select value={cluster} onChange={(e) => onCluster(e.target.value)}>
          <option value="">Unassigned</option>
          {clusters.map((c) => (
            <option key={c.id} value={c.id}>
              {c.name}
            </option>
          ))}
        </Select>
      </Field>
      <Field label="Environment">
        <Select value={env} onChange={(e) => onEnv(e.target.value)}>
          <option value="">None</option>
          {envs.map((e) => (
            <option key={e.id} value={e.id}>
              {e.name}
            </option>
          ))}
        </Select>
      </Field>
    </div>
  );
}

function StepHeading({
  n,
  title,
  done,
}: {
  n: number;
  title: string;
  done?: boolean;
}) {
  return (
    <div className="mb-3 flex items-center gap-2">
      <span
        className={`grid size-6 place-items-center rounded-full text-xs font-semibold ${done ? "bg-ok text-bg" : "btn-primary text-white"}`}
      >
        {n}
      </span>
      <h3 className="font-semibold">{title}</h3>
    </div>
  );
}

/** Assisted onboarding: probe → confirm host key → credential → live job. */
function Assisted({ initialCluster }: { initialCluster: string }) {
  const me = useMe();
  const probe = useProbeNode();
  const onboard = useOnboardNode();
  const toast = useToast();
  const [host, setHost] = useState("");
  const [port, setPort] = useState(22);
  const [key, setKey] = useState<HostKey>();
  const [confirmed, setConfirmed] = useState(false);
  const [username, setUsername] = useState("");
  const [method, setMethod] = useState<"password" | "private_key">("password");
  const [secret, setSecret] = useState("");
  const [passphrase, setPassphrase] = useState("");
  const [sudoPassword, setSudoPassword] = useState("");
  const [cluster, setCluster] = useState(initialCluster);
  const [env, setEnv] = useState("");
  const [keep, setKeep] = useState(false);
  const [account, setAccount] = useState<TerminalAccount>(DEFAULT_ACCOUNT);
  const [jobId, setJobId] = useState<string>();

  if (me.data && !me.data.mfa) {
    return (
      <div className="rounded-lg border border-warn/40 bg-warn-soft p-4 text-warn">
        <div className="flex items-center gap-2 font-medium">
          <Lock className="size-4" /> MFA required
        </div>
        <p className="mt-1">
          Handing Forge a server login is a sensitive action, so it needs an
          MFA-verified session.{" "}
          <Link
            to="/profile/$section"
            params={{ section: "security" }}
            className="underline"
          >
            Enable MFA
          </Link>
          , sign out and sign in again with your code. The manual installer
          works without it.
        </p>
      </div>
    );
  }

  if (jobId) return <OnboardingRun jobId={jobId} />;

  return (
    <div className="space-y-6">
      <section>
        <StepHeading n={1} title="Where is the machine?" done={!!key} />
        <form
          className="flex flex-wrap items-end gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            setKey(undefined);
            setConfirmed(false);
            probe.mutate(
              { host, port },
              {
                onSuccess: (r) => setKey(r.host_key),
                onError: (err) => toast(errorText(err), "error"),
              },
            );
          }}
        >
          <div className="min-w-56 flex-1">
            <Field label="Host or IP">
              <Input
                required
                value={host}
                onChange={(e) => setHost(e.target.value)}
                placeholder="203.0.113.10 or vm1.example.com"
              />
            </Field>
          </div>
          <div className="w-24">
            <Field label="SSH port">
              <Input
                type="number"
                min={1}
                max={65535}
                value={port}
                onChange={(e) => setPort(Number(e.target.value))}
              />
            </Field>
          </div>
          <Button disabled={probe.isPending}>
            <Fingerprint className="size-4" />{" "}
            {probe.isPending ? "Connecting…" : "Check host"}
          </Button>
        </form>
        <p className="mt-2 text-xs text-faint">
          Forge connects without logging in and shows you the server's SSH host
          key, so you can confirm it is the right machine.
        </p>
      </section>

      {key && (
        <section>
          <StepHeading n={2} title="Confirm the host key" done={confirmed} />
          <div className="rounded-md border border-line bg-panel-2 p-3">
            <div className="text-xs text-muted">{key.algorithm}</div>
            <div className="font-mono text-sm break-all">{key.fingerprint}</div>
          </div>
          <label className="mt-3 flex items-start gap-2">
            <input
              type="checkbox"
              className="mt-1"
              checked={confirmed}
              onChange={(e) => setConfirmed(e.target.checked)}
            />
            <span>
              This fingerprint matches my server (check with{" "}
              <code className="font-mono text-xs">
                ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub
              </code>
              ). Forge will refuse any other key.
            </span>
          </label>
        </section>
      )}

      {key && confirmed && (
        <form
          className="space-y-6"
          onSubmit={(e) => {
            e.preventDefault();
            onboard.mutate(
              {
                host,
                port,
                username,
                host_key_fingerprint: key.fingerprint,
                auth:
                  method === "password"
                    ? { type: "password", password: secret }
                    : {
                        type: "private_key",
                        pem: secret,
                        passphrase: passphrase || undefined,
                      },
                sudo_password: sudoPassword || undefined,
                cluster_id: cluster || undefined,
                environment_id: env || undefined,
                keep_credential: keep,
                terminal_account: accountName(account),
                terminal_account_sudo: account.sudo,
              },
              {
                onSuccess: (r) => {
                  setSecret("");
                  setSudoPassword("");
                  setJobId(r.job.id);
                },
                onError: (err) => toast(errorText(err), "error"),
              },
            );
          }}
        >
          <section>
            <StepHeading n={3} title="Login for a sudo-capable account" />
            <div className="grid gap-3 sm:grid-cols-2">
              <Field
                label="Username"
                hint="A normal user that can run sudo. Root login is not needed."
              >
                <Input
                  required
                  value={username}
                  onChange={(e) => setUsername(e.target.value)}
                  placeholder="ubuntu"
                  autoComplete="off"
                />
              </Field>
              <Field label="Authentication">
                <Select
                  value={method}
                  onChange={(e) =>
                    setMethod(e.target.value as "password" | "private_key")
                  }
                >
                  <option value="password">Password</option>
                  <option value="private_key">SSH private key</option>
                </Select>
              </Field>
            </div>
            <div className="mt-3 space-y-3">
              {method === "password" ? (
                <Field label="Password">
                  <Input
                    required
                    type="password"
                    value={secret}
                    onChange={(e) => setSecret(e.target.value)}
                    autoComplete="new-password"
                  />
                </Field>
              ) : (
                <>
                  <Field label="Private key (OpenSSH format)">
                    <TextArea
                      required
                      rows={5}
                      value={secret}
                      onChange={(e) => setSecret(e.target.value)}
                      placeholder="-----BEGIN OPENSSH PRIVATE KEY-----"
                      spellCheck={false}
                    />
                  </Field>
                  <Field label="Key passphrase (if any)">
                    <Input
                      type="password"
                      value={passphrase}
                      onChange={(e) => setPassphrase(e.target.value)}
                      autoComplete="new-password"
                    />
                  </Field>
                </>
              )}
              <Field
                label="Sudo password"
                hint={
                  method === "password"
                    ? "Leave empty if it is the same as the login password, or sudo needs none."
                    : "Needed unless the user has passwordless sudo."
                }
              >
                <Input
                  type="password"
                  value={sudoPassword}
                  onChange={(e) => setSudoPassword(e.target.value)}
                  autoComplete="new-password"
                />
              </Field>
            </div>
          </section>

          <section>
            <StepHeading n={4} title="Placement" />
            <Placement
              cluster={cluster}
              env={env}
              onCluster={setCluster}
              onEnv={setEnv}
            />
          </section>

          <section>
            <StepHeading n={5} title="Terminal access" />
            <TerminalAccountFields value={account} onChange={setAccount} />
            <p className="mt-2 text-xs text-faint">
              <span className="font-mono">{username || "The SSH user"}</span> is
              also saved as a terminal account for this node.
            </p>
          </section>

          <section className="rounded-lg border border-line bg-panel-2/60 p-4">
            <div className="flex items-center gap-2 font-medium">
              <KeyRound className="size-4 text-accent" /> How the credential is
              handled
            </div>
            <ul className="mt-2 list-disc space-y-1 pl-5 text-muted">
              <li>
                Sealed with envelope encryption in the vault; never shown again
                or sent to the browser.
              </li>
              <li>
                Used once by the onboarding job to install the agent, then{" "}
                <strong>deleted automatically</strong> (or after 1 hour at
                most).
              </li>
              <li>
                After that, the node is managed only through its agent's mTLS
                certificate — not SSH.
              </li>
            </ul>
            <label className="mt-3 flex items-start gap-2">
              <input
                type="checkbox"
                className="mt-1"
                checked={keep}
                onChange={(e) => setKeep(e.target.checked)}
              />
              <span>
                Keep the credential for repairs{" "}
                <span className="text-warn">(less secure)</span>
                <span className="block text-xs text-faint">
                  You can delete it later from the credential vault.
                </span>
              </span>
            </label>
          </section>

          <div className="flex justify-end">
            <Button variant="primary" disabled={onboard.isPending}>
              <Wand2 className="size-4" />{" "}
              {onboard.isPending ? "Starting…" : "Install agent and join"}
            </Button>
          </div>
        </form>
      )}
    </div>
  );
}

/** A running assisted onboarding: steps, error and the live install console. */
function OnboardingRun({ jobId }: { jobId: string }) {
  const job = useJob(jobId).data;
  const serverId =
    typeof job?.state?.server_id === "string" ? job.state.server_id : undefined;
  return (
    <div className="space-y-4">
      <p className="flex items-center gap-2 text-[13px] text-muted">
        <ShieldCheck className="size-4 shrink-0 text-ok" /> The credential is
        sealed in the vault and used only by this job. You can leave this page;
        the install continues and stays under Nodes and Jobs.
      </p>
      <JobDetail id={jobId} consoleHeight="h-96" />
      <div className="flex flex-wrap justify-end gap-2">
        <Link to="/nodes">
          <Button>Go to nodes</Button>
        </Link>
        {serverId && job?.status === "succeeded" && (
          <Link to="/nodes/$nodeId" params={{ nodeId: serverId }}>
            <Button variant="primary">
              Open node <ArrowRight className="size-4" />
            </Button>
          </Link>
        )}
      </div>
    </div>
  );
}

function Countdown({ until }: { until: number }) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  const s = Math.max(0, Math.round((until - now) / 1000));
  return (
    <span className={s < 120 ? "text-warn" : "text-muted"}>
      {s > 0 ? `expires in ${Math.floor(s / 60)}m ${s % 60}s` : "expired"}
    </span>
  );
}

/** The scheme, host and port of a URL (the URL itself when it does not parse). */
function originOf(u: string): string {
  try {
    return new URL(u).origin;
  } catch {
    return u;
  }
}

/** Manual onboarding: one-time token + one-line installer. */
function Manual({ initialCluster }: { initialCluster: string }) {
  const info = useOnboardingInfo();
  const create = useCreateEnrollmentToken();
  const toast = useToast();
  const [env, setEnv] = useState("");
  const [cluster, setCluster] = useState(initialCluster);
  const [ttl, setTtl] = useState(60);
  const [hostname, setHostname] = useState("");
  const [account, setAccount] = useState<TerminalAccount>(DEFAULT_ACCOUNT);
  const [token, setToken] = useState<EnrollmentToken>();
  const progress = useOnboardingConsole({ tokenId: token?.id }, !!token);
  const enrolled = progress.data?.server_id ?? undefined;
  const url = info.data?.install_url;
  // The token is read at a prompt and handed to sudo through the environment,
  // so it never lands in argv, shell history or the screen.
  const name = accountName(account);
  const installUrl =
    url && name
      ? `${url}${url.includes("?") ? "&" : "?"}terminal_account=${encodeURIComponent(name)}${account.sudo ? "&terminal_account_sudo=1" : ""}`
      : url;
  const linux = installUrl
    ? `printf 'Enrollment token: '; stty -echo; read -r FORGE_ENROLL_TOKEN; stty echo; echo; export FORGE_ENROLL_TOKEN; ` +
      `curl -fsSL '${installUrl.replaceAll("'", "'\\''")}' | sudo --preserve-env=FORGE_ENROLL_TOKEN sh; unset FORGE_ENROLL_TOKEN`
    : "";

  return (
    <div className="space-y-6">
      {info.data && !info.data.agents_available && (
        <div className="rounded-md border border-warn/40 bg-warn-soft px-3 py-2 text-warn">
          No agent builds are published on this control plane yet
          (agent_dist_dir). The installer will fail until they are added.
        </div>
      )}
      <section>
        <StepHeading n={1} title="Create a one-time token" done={!!token} />
        <form
          className="space-y-3"
          onSubmit={(e) => {
            e.preventDefault();
            create.mutate(
              {
                ttl_minutes: ttl,
                environment_id: env || undefined,
                cluster_id: cluster || undefined,
                hostname: hostname.trim() || undefined,
              },
              {
                onSuccess: setToken,
                onError: (err) => toast(errorText(err), "error"),
              },
            );
          }}
        >
          <Placement
            cluster={cluster}
            env={env}
            onCluster={setCluster}
            onEnv={setEnv}
          />
          <div className="flex flex-wrap items-end gap-2">
            <div className="w-56">
              <Field label="Hostname (optional)">
                <Input
                  value={hostname}
                  maxLength={253}
                  onChange={(e) => setHostname(e.target.value)}
                  placeholder="db-1"
                  spellCheck={false}
                  autoComplete="off"
                />
              </Field>
            </div>
            <div className="w-40">
              <Field label="Valid for (minutes)">
                <Input
                  type="number"
                  min={1}
                  max={1440}
                  value={ttl}
                  onChange={(e) => setTtl(Number(e.target.value))}
                />
              </Field>
            </div>
            <Button variant="primary" disabled={create.isPending}>
              Generate token
            </Button>
          </div>
          <p className="text-xs text-faint">
            The node joins the chosen cluster as soon as it enrolls. The
            hostname labels it while it installs.
          </p>
          <TerminalAccountFields value={account} onChange={setAccount} />
        </form>
      </section>
      {token && (
        <section>
          <StepHeading n={2} title="Run on the machine" />
          {url && (
            <p className="mb-2 text-xs text-faint">
              The commands use the address you opened the console on (
              <code>{originOf(url)}</code>). Nodes on another network: open the
              console via the address they reach, or edit the command.
            </p>
          )}
          <p className="mb-2 text-muted">
            Token: single-use and <Countdown until={token.expires_at} />. Paste
            it when the command asks for <code>Enrollment token</code>; it is
            not shown or kept in shell history.
          </p>
          <div className="flex items-start gap-2">
            <pre className="min-w-0 flex-1 overflow-x-auto rounded-md bg-term p-3 font-mono text-xs text-term-fg select-all">
              {token.token}
            </pre>
            <CopyButton text={token.token} />
          </div>
          <p className="mt-4 mb-2 text-muted">
            Linux (any distribution, with or without systemd):
          </p>
          <div className="flex items-start gap-2">
            <pre className="min-w-0 flex-1 overflow-x-auto rounded-md bg-term p-3 font-mono text-xs text-term-fg select-all">
              {linux}
            </pre>
            <CopyButton text={linux} />
          </div>
          <p className="mt-1 text-xs text-faint">
            The installer verifies the agent's SHA-256, creates an unprivileged{" "}
            <code>forge-agent</code> user and starts the service. The node
            appears under Nodes within seconds.
          </p>
          <p className="mt-4 mb-2 text-muted">
            Windows 10/11 or Server 2019+ (PowerShell as Administrator):
          </p>
          <div className="flex items-start gap-2">
            <pre className="min-w-0 flex-1 overflow-x-auto rounded-md bg-term p-3 font-mono text-xs text-term-fg select-all">
              {token.install.windows}
            </pre>
            <CopyButton text={token.install.windows} />
          </div>
          <p className="mt-1 text-xs text-faint">
            The installer checks the agent's SHA-256, installs it as the{" "}
            <code>ForgeAgent</code> service (automatic start) and enrolls the
            node. Run it again to upgrade; <code>-Uninstall</code> removes it.
          </p>
        </section>
      )}
      {token && (
        <section>
          <StepHeading n={3} title="Follow the install" done={!!enrolled} />
          <TokenConsole tokenId={token.id} height="h-80" />
          {progress.data?.error && (
            <p className="mt-2 rounded-md bg-bad-soft px-3 py-1.5 font-mono text-[11.5px] text-bad">
              {progress.data.error}
            </p>
          )}
          {enrolled && (
            <div className="mt-3 flex justify-end">
              <Link to="/nodes/$nodeId" params={{ nodeId: enrolled }}>
                <Button variant="primary">
                  Open node <ArrowRight className="size-4" />
                </Button>
              </Link>
            </div>
          )}
        </section>
      )}
    </div>
  );
}

export function AddNode() {
  const initialCluster =
    new URLSearchParams(location.search).get("cluster") ?? "";
  const tab =
    "rounded-md px-3 py-1.5 text-muted data-[state=active]:bg-panel data-[state=active]:font-medium data-[state=active]:text-accent data-[state=active]:shadow-sm";
  return (
    <>
      <PageHeader
        eyebrow="Infrastructure / Nodes"
        title="Add a node"
        description="Bring a Windows or Linux machine under management. Forge installs its agent; from then on the agent manages the node."
      />
      <Panel>
        <Tabs.Root defaultValue="assisted">
          <Tabs.List
            className="m-4 inline-flex gap-1 rounded-lg bg-panel-2 p-1"
            aria-label="Onboarding method"
          >
            <Tabs.Trigger value="assisted" className={tab}>
              <Wand2 className="mr-1.5 inline size-4" /> Assisted (SSH login)
            </Tabs.Trigger>
            <Tabs.Trigger value="manual" className={tab}>
              <Terminal className="mr-1.5 inline size-4" /> Manual (one-line
              installer)
            </Tabs.Trigger>
          </Tabs.List>
          <div className="border-t border-line p-5">
            <Tabs.Content value="assisted">
              <Assisted initialCluster={initialCluster} />
            </Tabs.Content>
            <Tabs.Content value="manual">
              <Manual initialCluster={initialCluster} />
            </Tabs.Content>
          </div>
        </Tabs.Root>
      </Panel>
    </>
  );
}
