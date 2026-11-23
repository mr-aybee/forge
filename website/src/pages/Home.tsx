import type { ReactNode } from "react";
import { ArrowRight, BookOpen, CheckCircle2, Download, Sparkles } from "lucide-react";
import { ENGINES, FEATURES, SITE, STATS, USE_CASES, docs } from "../content/site";
import { Button, Container, Eyebrow, SectionHeading } from "../components/ui";
import { Icon } from "../components/Icon";
import { FleetMap } from "../illustrations/FleetMap";
import { AppStoreScene, AuditScene, CountUp, FilesScene, McpScene, RolloutScene, TerminalScene } from "../illustrations/Scenes";
import { Checkpoints, Journey, Sparks } from "../illustrations/Art";

export function slug(s: string) {
  return s
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/(^-|-$)/g, "");
}

function Hero() {
  return (
    <section className="relative overflow-hidden">
      <div className="aurora pointer-events-none absolute inset-0" />
      <div className="grid-bg pointer-events-none absolute inset-0 opacity-60" />
      <Container className="relative grid items-center gap-10 pt-16 pb-10 lg:grid-cols-[1fr_1.05fr] lg:pt-24 lg:pb-16">
        <div>
          <a
            href="/download/"
            className="mb-7 inline-flex items-center gap-2 rounded-full border border-line bg-raised/80 py-1 pr-3 pl-1 text-xs text-muted shadow-card backdrop-blur hover:border-accent"
          >
            <span className="inline-flex items-center gap-1 rounded-full bg-ember-soft px-2 py-0.5 font-semibold text-ember">
              <Sparkles className="size-3" aria-hidden="true" /> Free
            </span>
            Download for Linux and Windows
            <ArrowRight className="size-3" aria-hidden="true" />
          </a>
          <h1 className="text-[2.6rem] leading-[1.05] font-semibold tracking-tight text-balance sm:text-6xl">
            Forge your fleet into <span className="text-gradient">one calm control center.</span>
          </h1>
          <p className="mt-6 max-w-xl text-lg text-pretty text-muted">
            Orbyn Forge is a lightweight, self-hosted control plane for Windows and Linux. Deploy applications, open terminals and files, and keep
            a tamper-evident record of every change, without opening a single inbound port on your servers.
          </p>
          <div className="mt-8 flex flex-wrap gap-3">
            <Button href="/download/">
              <Download className="size-4" aria-hidden="true" /> Download Forge
            </Button>
            <Button href={docs("getting-started/quickstart/")} variant="secondary">
              <BookOpen className="size-4" aria-hidden="true" /> Read the quickstart
            </Button>
          </div>
          <ul className="mt-8 flex flex-wrap gap-x-6 gap-y-2 text-sm text-muted">
            {["Self-hosted", "Agents dial out", "Free for commercial use"].map((t) => (
              <li key={t} className="inline-flex items-center gap-1.5">
                <CheckCircle2 className="size-4 text-ok" aria-hidden="true" /> {t}
              </li>
            ))}
          </ul>
        </div>
        <div className="relative mx-auto w-full max-w-[600px]">
          <FleetMap />
        </div>
      </Container>
    </section>
  );
}

function Stats() {
  return (
    <section className="relative">
      <Container>
        <div className="card grid grid-cols-2 divide-line md:grid-cols-4 md:divide-x">
          {STATS.map((s) => (
            <div key={s.label} className="px-4 py-7 text-center">
              <CountUp value={s.value} className="text-3xl font-semibold tracking-tight text-fg tabular-nums sm:text-4xl" />
              <p className="mt-1 text-sm text-muted">{s.label}</p>
            </div>
          ))}
        </div>
      </Container>
    </section>
  );
}

function Engines() {
  const row = [...ENGINES, ...ENGINES];
  return (
    <section className="py-16">
      <Container>
        <p className="mb-6 text-center text-sm text-muted">An orchestration layer. Forge drives the engines you already trust.</p>
        <div className="marquee overflow-hidden">
          <ul className="flex w-max animate-[marquee_40s_linear_infinite] gap-3 hover:[animation-play-state:paused]">
            {row.map((e, i) => (
              <li key={`${e}-${i}`} aria-hidden={i >= ENGINES.length} className="rounded-full border border-line bg-raised px-4 py-2 text-sm whitespace-nowrap text-muted shadow-card">
                {e}
              </li>
            ))}
          </ul>
        </div>
      </Container>
    </section>
  );
}

function Tile({ title, body, doc, children, className = "" }: { title: string; body: string; doc: string; children: ReactNode; className?: string }) {
  return (
    <a href={docs(doc)} className={`group card flex flex-col overflow-hidden transition-transform hover:-translate-y-0.5 hover:border-accent ${className}`}>
      <div className="flex-1 border-b border-line bg-sunken/60 p-5">{children}</div>
      <div className="p-6">
        <h3 className="flex items-center justify-between font-semibold">
          {title}
          <ArrowRight className="size-4 text-accent opacity-0 transition-opacity group-hover:opacity-100" aria-hidden="true" />
        </h3>
        <p className="mt-1.5 text-sm text-pretty text-muted">{body}</p>
      </div>
    </a>
  );
}

function Bento() {
  return (
    <section className="py-20">
      <Container>
        <SectionHeading
          eyebrow="What you get"
          title="Everything a fleet needs, nothing you have to babysit."
          lead="One login, one permission model, one audit trail and one view of your applications, across every server you run."
        />
        <div className="grid gap-5 md:grid-cols-6">
          <Tile className="md:col-span-3" title="Browser terminal" doc="guides/add-nodes/" body="Full SSH in the browser with 5-minute certificates from Forge's own CA. No stored keys, no shared passwords.">
            <TerminalScene />
          </Tile>
          <Tile className="md:col-span-3" title="Releases & rollouts" doc="concepts/deployments/" body="Rolling, blue-green or canary, with health gates, approvals and automatic rollback. Resumes after a crash.">
            <RolloutScene />
          </Tile>
          <Tile className="md:col-span-2" title="App Store" doc="guides/app-store/" body="Databases, runtimes and web servers on one node or many. Generated passwords stay on the node.">
            <AppStoreScene />
          </Tile>
          <Tile className="md:col-span-2" title="AI agents, with a human in the loop" doc="concepts/mcp-connectors/" body="Claude, ChatGPT, Cursor and more act with your permissions. Destructive actions wait for you.">
            <McpScene />
          </Tile>
          <Tile className="md:col-span-2" title="File explorer & SFTP" doc="concepts/file-explorer/" body="Edit files on Linux, Windows and containers. Hand out expiring SFTP access without node accounts.">
            <FilesScene />
          </Tile>
          <Tile className="md:col-span-6" title="A signed, hash-chained audit log" doc="concepts/audit/" body="Every action, and every denial, is sealed into a chain that shows edits and deletions. Prove who did what, and when.">
            <div className="mx-auto max-w-xl py-2">
              <AuditScene />
            </div>
          </Tile>
        </div>
        <div className="mt-10 text-center">
          <Button href="/features/" variant="secondary">
            Explore all {FEATURES.length} features <ArrowRight className="size-4" aria-hidden="true" />
          </Button>
        </div>
      </Container>
    </section>
  );
}

function HowItWorks() {
  return (
    <section className="relative overflow-hidden border-y border-line bg-sunken py-20">
      <Container>
        <SectionHeading center eyebrow="How it works" title="Up and running before your coffee cools." lead="One control plane, small agents that dial out, and a console for everything else." />
        <Journey />
      </Container>
    </section>
  );
}

function SecurityBand() {
  return (
    <section className="py-20">
      <Container>
        <div className="relative overflow-hidden rounded-[28px] bg-gradient-to-br from-[#0f3550] to-[#0a2234] px-6 py-14 text-white sm:px-12">
          <div className="pointer-events-none absolute -top-24 -right-24 size-80 rounded-full bg-[#4cc3dd]/20 blur-3xl" />
          <div className="relative grid items-center gap-10 lg:grid-cols-[1fr_1.5fr]">
            <div>
              <p className="mb-3 text-xs font-semibold tracking-[0.16em] text-[#8fdcf0] uppercase">Security</p>
              <h2 className="text-3xl font-semibold tracking-tight text-balance sm:text-4xl">Checked at every door, not just the front one.</h2>
              <p className="mt-4 text-white/70">
                A request that passes the API is checked again by the job runner, the agent hub and the agent on the node. A bug in one layer does not open the others.
              </p>
              <a href="/security/" className="mt-6 inline-flex items-center gap-1.5 font-medium text-[#8fdcf0] hover:underline">
                How Forge protects your fleet <ArrowRight className="size-4" aria-hidden="true" />
              </a>
            </div>
            <div className="rounded-2xl border border-white/10 bg-white/[0.03] p-4">
              <Checkpoints />
            </div>
          </div>
        </div>
      </Container>
    </section>
  );
}

function UseCaseStrip() {
  return (
    <section className="pb-20">
      <Container>
        <SectionHeading eyebrow="Use cases" title="Built for the teams that keep things running." />
        <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {USE_CASES.map((u) => (
            <a key={u.title} href={`/use-cases/#${slug(u.title)}`} className="group card p-6 transition-transform hover:-translate-y-0.5 hover:border-accent">
              <span className="inline-flex size-10 items-center justify-center rounded-xl bg-accent-soft text-accent">
                <Icon name={u.icon} className="size-5" />
              </span>
              <p className="mt-4 text-xs font-medium tracking-wider text-muted uppercase">{u.who}</p>
              <h3 className="mt-1 font-semibold">{u.title}</h3>
              <span className="mt-3 inline-flex items-center gap-1 text-sm text-accent opacity-80 group-hover:opacity-100">
                Read the story <ArrowRight className="size-3.5" aria-hidden="true" />
              </span>
            </a>
          ))}
        </div>
      </Container>
    </section>
  );
}

export function CallToAction() {
  return (
    <section className="pb-24">
      <Container>
        <div className="card relative overflow-hidden px-6 pt-16 pb-24 text-center sm:px-12">
          <Sparks />
          <div className="relative">
            <Eyebrow>Free for everyone</Eyebrow>
            <h2 className="text-3xl font-semibold tracking-tight text-balance sm:text-5xl">Light the forge.</h2>
            <p className="mx-auto mt-4 max-w-xl text-lg text-muted">
              Download Forge and run your fleet from one place, including for commercial work. Install it in minutes.
            </p>
            <div className="mt-8 flex flex-wrap justify-center gap-3">
              <Button href="/download/">
                <Download className="size-4" aria-hidden="true" /> Download Forge
              </Button>
              <Button href={SITE.docs} variant="secondary">
                <BookOpen className="size-4" aria-hidden="true" /> Browse the docs
              </Button>
            </div>
          </div>
        </div>
      </Container>
    </section>
  );
}

export function Home() {
  return (
    <>
      <Hero />
      <Stats />
      <Engines />
      <Bento />
      <HowItWorks />
      <SecurityBand />
      <UseCaseStrip />
      <CallToAction />
    </>
  );
}
