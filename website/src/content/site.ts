// Every link, figure and piece of copy the website shows lives here, so the
// site has one place to update when the product changes. Keep figures in
// sync with BENCHMARKS.md and features with README.md.

export const SITE = {
  name: "Orbyn Forge",
  url: "https://forge.orbyntech.com",
  company: "Orbyn Technologies",
  companyUrl: "https://orbyntech.com",
  releasesRepo: "mr-aybee/forge",
  releasesUrl: "https://github.com/mr-aybee/forge/releases",
  latestUrl: "https://github.com/mr-aybee/forge/releases/latest",
  issuesUrl: "https://github.com/mr-aybee/forge/issues",
  linkedin: "https://www.linkedin.com/company/orbyn-technologies/",
  email: "contact@orbyntech.com",
  docs: "/docs/",
} as const;

export const docs = (path: string) => `/docs/${path.replace(/^\//, "")}`;

export type Route = {
  path: string;
  title: string;
  description: string;
  nav?: string;
};

export const ROUTES: Route[] = [
  {
    path: "/",
    title: "Orbyn Forge: one secure control center for Windows and Linux",
    description:
      "Self-hosted control plane for Windows and Linux fleets. Deploy apps, run containers, open terminals and audit every change. Free to download.",
  },
  {
    path: "/features/",
    nav: "Features",
    title: "Features | Orbyn Forge",
    description:
      "Clusters, applications, App Store, containers, releases, file explorer, object storage, alerts, logs, CLI and MCP: everything Forge does.",
  },
  {
    path: "/use-cases/",
    nav: "Use cases",
    title: "Use cases | Orbyn Forge",
    description:
      "How teams use Forge: hybrid fleet management, self-hosted PaaS, safe releases, managed service providers and AI-assisted operations.",
  },
  {
    path: "/security/",
    nav: "Security",
    title: "Security | Orbyn Forge",
    description:
      "Cedar authorization at every layer, MFA, short-lived SSH certificates, mutual TLS agents and a signed, hash-chained audit log.",
  },
  {
    path: "/download/",
    nav: "Download",
    title: "Download | Orbyn Forge",
    description: "Download Orbyn Forge for Linux and Windows. Open source under Apache-2.0.",
  },
  {
    path: "/license/",
    title: "License | Orbyn Forge",
    description: "Orbyn Forge is open source under the Apache License 2.0.",
  },
];

export const NOT_FOUND: Route = {
  path: "/404",
  title: "Page not found | Orbyn Forge",
  description: "This page does not exist.",
};

export const STATS = [
  { value: "17 MB", label: "RAM for an idle agent" },
  { value: "1,000", label: "agents on one control plane" },
  { value: "87 MB", label: "control-plane RAM at 1,000 agents" },
  { value: "0", label: "inbound ports needed on nodes" },
];

export type Feature = {
  icon: string;
  title: string;
  body: string;
  doc: string;
  group: FeatureGroup;
};

export type FeatureGroup = "Operate" | "Deploy" | "Access" | "Observe" | "Extend";

export const FEATURE_GROUPS: { id: FeatureGroup; blurb: string }[] = [
  { id: "Operate", blurb: "Model your estate and run it from one console." },
  { id: "Deploy", blurb: "Ship software to the nodes you choose, safely." },
  { id: "Access", blurb: "Reach machines, files and data without handing out keys." },
  { id: "Observe", blurb: "Know what is happening and who did it." },
  { id: "Extend", blurb: "Automate Forge from scripts, CI, plugins and AI agents." },
];

export const FEATURES: Feature[] = [
  {
    group: "Operate",
    icon: "Boxes",
    title: "Clusters, applications & environments",
    body: "Group machines into clusters. Model each product as an application with test, staging and production environments, and the services that run inside them.",
    doc: "concepts/applications-and-environments/",
  },
  {
    group: "Operate",
    icon: "Rocket",
    title: "Easy onboarding",
    body: "Give Forge an SSH login and it installs the agent for you, or run a one-line installer. Credentials are encrypted, used once and deleted.",
    doc: "guides/add-nodes/",
  },
  {
    group: "Operate",
    icon: "Store",
    title: "App Store",
    body: "Install Nginx, PostgreSQL, MariaDB, Redis, MongoDB, SQL Server, Node.js, Python, .NET, Java, Go, Docker, RustFS and more on one node or many.",
    doc: "guides/app-store/",
  },
  {
    group: "Operate",
    icon: "RefreshCw",
    title: "Agent updates",
    body: "Bring every agent up to the control plane's version. Updates are verified, and a node rolls back on its own if the new agent does not come back.",
    doc: "concepts/agents/",
  },
  {
    group: "Deploy",
    icon: "Container",
    title: "Containers & Compose",
    body: "Manage Docker containers and images on any node. Deploy Compose stacks across nodes from a visual preview, with internal ports by default.",
    doc: "guides/containers/",
  },
  {
    group: "Deploy",
    icon: "Ship",
    title: "Releases & rollouts",
    body: "Describe a release in forge.deploy.yaml. Roll it out with rolling, blue-green or canary strategies, health gates, approvals and automatic rollback.",
    doc: "concepts/deployments/",
  },
  {
    group: "Deploy",
    icon: "Package",
    title: "Applications",
    body: "Run services, scheduled jobs and containers with first-class environment variables, sealed secrets and active/standby placement.",
    doc: "guides/applications/",
  },
  {
    group: "Deploy",
    icon: "Globe",
    title: "Sites & DNS",
    body: "Route domains through Nginx edge nodes with TLS, standby upstreams and rate limits, and publish records on PowerDNS.",
    doc: "guides/cluster-network/",
  },
  {
    group: "Access",
    icon: "SquareTerminal",
    title: "Browser terminal",
    body: "A full SSH terminal with tabs, split view and search, using 5-minute certificates from Forge's SSH CA. No stored passwords or keys.",
    doc: "guides/add-nodes/",
  },
  {
    group: "Access",
    icon: "FolderOpen",
    title: "File explorer",
    body: "Browse, edit, upload and restore files on Linux, Windows and containers. Hand out expiring SFTP access without node accounts.",
    doc: "concepts/file-explorer/",
  },
  {
    group: "Access",
    icon: "Database",
    title: "Object storage",
    body: "Run RustFS as a managed S3 store or connect AWS S3, R2, Wasabi, B2 or MinIO. Issue keys limited to one bucket.",
    doc: "concepts/object-storage/",
  },
  {
    group: "Access",
    icon: "Cable",
    title: "Port tunnels",
    body: "Reach a service's port from your laptop through the API port alone. Every tunnel is authorized, audited, listed and revocable.",
    doc: "concepts/port-tunnels/",
  },
  {
    group: "Observe",
    icon: "Siren",
    title: "Alerts & notifications",
    body: "Alarms for offline nodes, high CPU, memory or disk and failed health checks, sent to webhooks, chat or email, with silences and maintenance windows.",
    doc: "concepts/alerts/",
  },
  {
    group: "Observe",
    icon: "ScrollText",
    title: "Application logs",
    body: "Send structured logs from any Seq client. Search by text or property, follow a live tail and see counts per level over time.",
    doc: "guides/application-logs/",
  },
  {
    group: "Observe",
    icon: "FileCheck2",
    title: "Durable jobs & audit",
    body: "Every operation is a job that survives restarts. Every action, including denials, lands in a hash-chained, signed audit log.",
    doc: "concepts/audit/",
  },
  {
    group: "Observe",
    icon: "Activity",
    title: "Analytics",
    body: "Node history and application metrics in a dedicated analytics database with its own retention.",
    doc: "concepts/analytics/",
  },
  {
    group: "Extend",
    icon: "Terminal",
    title: "forge CLI",
    body: "Manage nodes, stacks, apps, files and the audit log from a terminal or pipeline. JSON output, stable exit codes, MFA step-up.",
    doc: "reference/cli/",
  },
  {
    group: "Extend",
    icon: "Bot",
    title: "AI agents (MCP)",
    body: "Connect Claude, ChatGPT, Gemini CLI, Codex or Cursor. Agents act with your permissions; destructive actions stay with a person.",
    doc: "concepts/mcp-connectors/",
  },
  {
    group: "Extend",
    icon: "Puzzle",
    title: "Plugins",
    body: "Signed WebAssembly extensions in a strict sandbox with fuel, memory and time budgets.",
    doc: "concepts/plugins/",
  },
  {
    group: "Extend",
    icon: "KeyRound",
    title: "Sign-in & API keys",
    body: "Local accounts with TOTP or passkeys, OIDC single sign-on, OAuth + PKCE for apps, and scoped API keys for scripts and CI.",
    doc: "guides/access-profiles/",
  },
];

export type UseCase = {
  icon: string;
  title: string;
  who: string;
  problem: string;
  solution: string[];
  docs: { label: string; path: string }[];
};

export const USE_CASES: UseCase[] = [
  {
    icon: "Server",
    title: "Run a mixed Windows and Linux fleet",
    who: "IT and platform teams",
    problem:
      "Servers live in different data centres and clouds, run different operating systems, and are reached through a mix of RDP, SSH keys and jump hosts nobody fully tracks.",
    solution: [
      "Enroll every machine as a node; agents dial out, so no inbound ports open.",
      "Group nodes into clusters and give teams roles per cluster or environment.",
      "Open terminals and files from the browser with short-lived certificates.",
    ],
    docs: [
      { label: "Add nodes", path: "guides/add-nodes/" },
      { label: "Clusters", path: "concepts/clusters/" },
    ],
  },
  {
    icon: "Layers",
    title: "A self-hosted platform for your apps",
    who: "Product and engineering teams",
    problem:
      "You want the convenience of a PaaS, but your apps must run on your own servers, close to your data, under your own rules.",
    solution: [
      "Model each product as an application with test, staging and production.",
      "Install databases and runtimes from the App Store, or deploy Compose stacks.",
      "Route domains through Nginx edges with TLS and publish DNS on PowerDNS.",
    ],
    docs: [
      { label: "Applications", path: "guides/applications/" },
      { label: "Compose stacks", path: "guides/compose-stacks/" },
    ],
  },
  {
    icon: "GitBranch",
    title: "Ship releases without the 2 a.m. surprise",
    who: "DevOps and release engineers",
    problem:
      "Deployments are hand-run scripts. A failed step leaves half the servers on the new version and no record of who did what.",
    solution: [
      "Describe the release in forge.deploy.yaml and run it from CI.",
      "Choose rolling, blue-green or canary with error-rate and latency gates.",
      "Require approvals; Forge resumes after a crash and rolls back on failure.",
    ],
    docs: [
      { label: "Deployments", path: "concepts/deployments/" },
      { label: "Deployment YAML", path: "reference/deployment-yaml/" },
    ],
  },
  {
    icon: "Building2",
    title: "Manage clients' servers as an MSP",
    who: "Managed service providers and agencies",
    problem:
      "Each client has its own machines, people and compliance needs. Shared passwords and shared admin accounts do not survive an audit.",
    solution: [
      "One cluster per client, with roles that stop at the client boundary.",
      "Hand out expiring access to files, buckets and ports instead of accounts.",
      "Show clients a signed audit log of every action taken on their systems.",
    ],
    docs: [
      { label: "Authorization", path: "concepts/authorization/" },
      { label: "Audit", path: "concepts/audit/" },
    ],
  },
  {
    icon: "ShieldCheck",
    title: "Meet audit and compliance requirements",
    who: "Security and compliance owners",
    problem:
      "Auditors ask who accessed a production server last quarter, and why. The answer is spread across bastion logs, chat and memory.",
    solution: [
      "Every request is authorized by Cedar policy where it takes effect.",
      "MFA guards sensitive actions; SSH certificates expire in minutes.",
      "Every change and every denial lands in a tamper-evident audit log.",
    ],
    docs: [
      { label: "Security model", path: "concepts/security-model/" },
      { label: "Hardening", path: "security/hardening/" },
    ],
  },
  {
    icon: "Bot",
    title: "Let AI agents help, safely",
    who: "Teams adopting AI assistants",
    problem:
      "AI coding agents can run commands, but giving them SSH keys to production is not an option.",
    solution: [
      "Connect Claude, ChatGPT, Gemini CLI, Codex or Cursor through Forge's MCP endpoint.",
      "Approve each agent as read-only or auto; it never exceeds your permissions.",
      "Destructive and MFA-protected actions stay with a person, and all are audited.",
    ],
    docs: [
      { label: "MCP connectors", path: "concepts/mcp-connectors/" },
      { label: "Connect AI agents", path: "guides/connect-ai-agents/" },
    ],
  },
];

export const SECURITY_POINTS = [
  {
    icon: "Scale",
    title: "Authorized where it takes effect",
    body: "Cedar policies are checked in the API, the job runner, the agent hub, the agent itself and the plugin host. The UI is never trusted.",
  },
  {
    icon: "Fingerprint",
    title: "MFA for sensitive actions",
    body: "Opening a terminal or running a command asks for an MFA-verified session. TOTP and passkeys are supported.",
  },
  {
    icon: "Timer",
    title: "Short-lived credentials",
    body: "Terminal sessions use 5-minute SSH certificates from Forge's own CA. Onboarding credentials are used once and deleted.",
  },
  {
    icon: "Network",
    title: "Agents dial out over mTLS",
    body: "Each agent has its own certificate and connects out to the control plane. Commands are signed and replay-protected.",
  },
  {
    icon: "Link",
    title: "Tamper-evident audit",
    body: "Every mutation and every denial is recorded in a hash-chained, signed log that detects edits and deletions.",
  },
  {
    icon: "Lock",
    title: "Secrets stay put",
    body: "Secrets are encrypted in a vault, never placed in URLs, logs or the browser. Generated database passwords stay on the node.",
  },
];

export const ENGINES = ["Docker", "Nginx", "Caddy", "PowerDNS", "PostgreSQL", "MariaDB", "Redis", "MongoDB", "SQL Server", "RustFS", "Certbot", "Keepalived"];
