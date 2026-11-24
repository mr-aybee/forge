import { useEffect, useState } from "react";
import { ArrowRight, ArrowUpRight, BookOpen, ShieldCheck } from "lucide-react";
import { SITE, docs } from "../content/site";
import { Button, CodeBlock, Container, PageHero } from "../components/ui";
import { GithubMark } from "../components/Icon";

type Os = "linux" | "windows" | "cli";

const PLACEHOLDER = "<version>";

/**
 * Asks GitHub for the latest release tag so the commands can be pasted as is.
 * The pre-rendered page shows `<version>`; if the request fails it stays.
 */
function useLatestVersion() {
  const [version, setVersion] = useState<string | null>(null);
  useEffect(() => {
    const ctrl = new AbortController();
    fetch(`https://api.github.com/repos/${SITE.releasesRepo}/releases/latest`, {
      signal: ctrl.signal,
      headers: { Accept: "application/vnd.github+json" },
    })
      .then((r) => (r.ok ? r.json() : Promise.reject(new Error(String(r.status)))))
      .then((j: { tag_name?: string }) => {
        if (j.tag_name) setVersion(j.tag_name.replace(/^v/, ""));
      })
      .catch(() => {
        // Rate-limited, offline or no release yet: keep the placeholder.
      });
    return () => ctrl.abort();
  }, []);
  return version;
}

function commands(os: Os, v: string) {
  const base = `https://github.com/${SITE.releasesRepo}/releases/latest/download`;
  switch (os) {
    case "linux":
      return `curl -fLO ${base}/forge-${v}-x86_64-linux.tar.gz
tar xzf forge-${v}-x86_64-linux.tar.gz
cd forge-${v}-x86_64-linux
sudo ./install.sh --public-url http://forge.example.com:8080`;
    case "windows":
      return `# The forge CLI for Windows x86_64. The Windows control plane
# and agent are not in releases yet; build them from source.
$f = "forge-cli-x86_64-pc-windows-msvc.exe"
Invoke-WebRequest ${base}/$f -OutFile forge.exe
.\\forge.exe contexts add prod --server https://forge.example.com`;
    case "cli":
      return `# Linux x86_64; see the CLI docs for Windows
curl -fLO ${base}/forge-cli-x86_64-unknown-linux-musl
sudo install -m 0755 forge-cli-x86_64-unknown-linux-musl /usr/local/bin/forge
forge contexts add prod --server https://forge.example.com
forge auth login`;
  }
}

const TABS: { id: Os; label: string; note: string }[] = [
  { id: "linux", label: "Linux", note: "x86_64 and arm64 with systemd: Debian, Ubuntu, RHEL, Rocky, Alma and more." },
  { id: "windows", label: "Windows", note: "Releases ship only the forge CLI for Windows x86_64. The Windows control plane and agent build from source and are not yet verified." },
  { id: "cli", label: "forge CLI", note: "The command-line client for Linux and Windows. On macOS, build it from source." },
];

const PACKAGES = [
  ["forge-<version>-x86_64-linux.tar.gz", "Control plane for Linux x86_64"],
  ["forge-<version>-aarch64-linux.tar.gz", "Control plane for Linux arm64"],
  ["forge-cli-<target>", "forge CLI for Linux and Windows"],
  ["SHA256SUMS, SHA256SUMS.sig", "Checksums and their signature"],
  ["LICENSE.txt", "Apache License 2.0"],
];

export function Download() {
  const [os, setOs] = useState<Os>("linux");
  const version = useLatestVersion();
  const tab = TABS.find((t) => t.id === os)!;

  return (
    <>
      <PageHero
        eyebrow="Download"
        title="Get Orbyn Forge. It is free."
        lead="One self-contained package per platform: the control plane with the web console built in, the agents your nodes download, and an installer. No database or web server to set up."
      >
        <Button href={SITE.latestUrl} external>
          <GithubMark className="size-4" /> Latest release{version ? ` (v${version})` : ""}
        </Button>
        <Button href={docs("getting-started/installation/")} variant="secondary">
          <BookOpen className="size-4" aria-hidden="true" /> Installation guide
        </Button>
      </PageHero>

      <section className="py-16">
        <Container className="grid gap-10 lg:grid-cols-[1.4fr_1fr]">
          <div>
            <div role="tablist" aria-label="Platform" className="inline-flex rounded-full border border-line bg-sunken p-1">
              {TABS.map((t) => (
                <button
                  key={t.id}
                  role="tab"
                  type="button"
                  aria-selected={os === t.id}
                  aria-controls="install-panel"
                  onClick={() => setOs(t.id)}
                  className="rounded-full px-4 py-1.5 text-sm text-muted aria-selected:bg-raised aria-selected:font-medium aria-selected:text-fg aria-selected:shadow-sm"
                >
                  {t.label}
                </button>
              ))}
            </div>
            <div id="install-panel" role="tabpanel" className="mt-5">
              <p className="mb-4 text-sm text-muted">{tab.note}</p>
              <CodeBlock code={commands(os, version ?? PLACEHOLDER)} label={tab.label} />
              <p className="mt-4 text-sm text-muted">
                The installer prints the console address and a generated password for the first administrator. Continue with the{" "}
                <a className="text-accent hover:underline" href={docs("getting-started/quickstart/")}>
                  quickstart
                </a>
                .
              </p>
            </div>
          </div>

          <aside className="space-y-4">
            <div className="rounded-[var(--forge-radius)] border border-line bg-raised p-6">
              <ShieldCheck className="size-5 text-accent" aria-hidden="true" />
              <h2 className="mt-3 font-semibold">Verify your download</h2>
              <p className="mt-2 text-sm text-muted">
                Every release ships <code className="font-mono text-xs">SHA256SUMS</code> and a signature. Forge checks the same signature
                before it updates itself or its agents.
              </p>
              <a className="mt-4 inline-flex items-center gap-1 text-sm font-medium text-accent hover:underline" href={docs("getting-started/installation/#verify-a-download")}>
                How to verify <ArrowRight className="size-3.5" aria-hidden="true" />
              </a>
            </div>
            <div className="rounded-[var(--forge-radius)] border border-line bg-raised p-6">
              <h2 className="font-semibold">Requirements</h2>
              <ul className="mt-3 space-y-1.5 text-sm text-muted">
                <li>Control plane: Linux with systemd, or Windows Server 2019+ / Windows 10 / 11</li>
                <li>Database: embedded SQLite, or PostgreSQL if you prefer</li>
                <li>Ports: TCP 8080 (console and API) and 7443 (agents)</li>
                <li>Nodes: no inbound ports; agents dial out</li>
              </ul>
            </div>
          </aside>
        </Container>
      </section>

      <section className="border-t border-line bg-sunken py-16">
        <Container>
          <h2 className="text-2xl font-semibold tracking-tight">What is in a release</h2>
          <div className="mt-6 overflow-hidden rounded-[var(--forge-radius)] border border-line bg-raised">
            <table className="w-full text-left text-sm">
              <tbody className="divide-y divide-line">
                {PACKAGES.map(([file, what]) => (
                  <tr key={file}>
                    <td className="px-5 py-3 font-mono text-xs break-all">{file}</td>
                    <td className="px-5 py-3 text-muted">{what}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <div className="mt-6 flex flex-wrap gap-4 text-sm">
            <a href={SITE.releasesUrl} target="_blank" rel="noopener noreferrer" className="inline-flex items-center gap-1 text-accent hover:underline">
              All releases <ArrowUpRight className="size-3.5" aria-hidden="true" />
            </a>
            <a href={docs("reference/upgrade-notes/")} className="inline-flex items-center gap-1 text-accent hover:underline">
              Upgrade notes <ArrowRight className="size-3.5" aria-hidden="true" />
            </a>
            <a href="/license/" className="inline-flex items-center gap-1 text-accent hover:underline">
              License <ArrowRight className="size-3.5" aria-hidden="true" />
            </a>
          </div>
        </Container>
      </section>
    </>
  );
}
