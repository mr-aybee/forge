import { useState, type ReactNode } from "react";
import { ArrowUpRight, Menu, X } from "lucide-react";
import { GithubMark } from "./Icon";
import { ROUTES, SITE } from "../content/site";
import { Container } from "./ui";
import { ThemeToggle } from "./ThemeToggle";

const NAV = [
  ...ROUTES.filter((r) => r.nav && r.path !== "/download/").map((r) => ({ href: r.path, label: r.nav! })),
  { href: SITE.docs, label: "Docs" },
];

function Logo() {
  return (
    <a href="/" className="flex items-center gap-2.5 font-semibold tracking-tight" aria-label="Orbyn Forge home">
      <img src="/forge-mark.svg" alt="" width={30} height={30} className="size-[30px]" />
      <span>
        Orbyn <span className="text-accent">Forge</span>
      </span>
    </a>
  );
}

function isActive(current: string, href: string) {
  return href !== "/" && current.startsWith(href);
}

export function Header({ path }: { path: string }) {
  const [open, setOpen] = useState(false);
  return (
    <header className="sticky top-0 z-40 border-b border-line/70 bg-bg/75 backdrop-blur-xl">
      <Container className="flex h-16 items-center justify-between gap-4">
        <Logo />
        <nav className="hidden items-center gap-1 md:flex" aria-label="Main">
          {NAV.map((n) => (
            <a
              key={n.href}
              href={n.href}
              aria-current={isActive(path, n.href) ? "page" : undefined}
              className="rounded-full px-3 py-2 text-sm text-muted hover:text-fg aria-[current=page]:text-fg aria-[current=page]:font-medium"
            >
              {n.label}
            </a>
          ))}
        </nav>
        <div className="flex items-center gap-1">
          <ThemeToggle />
          <a
            href={SITE.releasesUrl}
            target="_blank"
            rel="noopener noreferrer"
            className="hidden size-9 items-center justify-center rounded-full text-muted hover:bg-sunken hover:text-fg sm:inline-flex"
            aria-label="Releases on GitHub"
          >
            <GithubMark className="size-4" />
          </a>
          <a
            href="/download/"
            className="ml-1 hidden h-9 items-center rounded-full bg-accent px-4 text-sm font-semibold text-on-accent hover:bg-accent-strong sm:inline-flex"
          >
            Download
          </a>
          <button
            type="button"
            className="inline-flex size-9 items-center justify-center rounded-full text-fg hover:bg-sunken md:hidden"
            aria-expanded={open}
            aria-controls="mobile-nav"
            aria-label={open ? "Close menu" : "Open menu"}
            onClick={() => setOpen((o) => !o)}
          >
            {open ? <X className="size-5" /> : <Menu className="size-5" />}
          </button>
        </div>
      </Container>
      {open && (
        <nav id="mobile-nav" className="border-t border-line md:hidden" aria-label="Mobile">
          <Container className="flex flex-col py-2">
            {[...NAV, { href: "/download/", label: "Download" }].map((n) => (
              <a key={n.href} href={n.href} className="rounded-lg px-2 py-3 text-base text-fg hover:bg-sunken">
                {n.label}
              </a>
            ))}
          </Container>
        </nav>
      )}
    </header>
  );
}

const FOOTER: { title: string; links: { href: string; label: string; external?: boolean }[] }[] = [
  {
    title: "Product",
    links: [
      { href: "/features/", label: "Features" },
      { href: "/use-cases/", label: "Use cases" },
      { href: "/security/", label: "Security" },
      { href: "/download/", label: "Download" },
    ],
  },
  {
    title: "Documentation",
    links: [
      { href: "/docs/getting-started/quickstart/", label: "Quickstart" },
      { href: "/docs/getting-started/installation/", label: "Installation" },
      { href: "/docs/concepts/architecture/", label: "Architecture" },
      { href: "/docs/reference/rest-api/", label: "REST API" },
    ],
  },
  {
    title: "Resources",
    links: [
      { href: SITE.releasesUrl, label: "Releases", external: true },
      { href: SITE.issuesUrl, label: "Report an issue", external: true },
      { href: "/docs/roadmap/", label: "Roadmap" },
      { href: "/license/", label: "License" },
    ],
  },
  {
    title: "Company",
    links: [
      { href: SITE.companyUrl, label: "Orbyn Technologies", external: true },
      { href: SITE.linkedin, label: "LinkedIn", external: true },
      { href: `mailto:${SITE.email}`, label: "Contact" },
    ],
  },
];

export function Footer() {
  return (
    <footer className="border-t border-line bg-sunken">
      <Container className="grid gap-10 py-14 md:grid-cols-[1.4fr_repeat(4,1fr)]">
        <div>
          <Logo />
          <p className="mt-4 max-w-xs text-sm text-muted">
            One secure control center for your Windows and Linux machines. Free to download and use.
          </p>
        </div>
        {FOOTER.map((col) => (
          <div key={col.title}>
            <p className="mb-3 text-sm font-semibold">{col.title}</p>
            <ul className="space-y-2">
              {col.links.map((l) => (
                <li key={l.label}>
                  <a
                    href={l.href}
                    {...(l.external ? { target: "_blank", rel: "noopener noreferrer" } : {})}
                    className="inline-flex items-center gap-1 text-sm text-muted hover:text-accent"
                  >
                    {l.label}
                    {l.external && <ArrowUpRight className="size-3" aria-hidden="true" />}
                  </a>
                </li>
              ))}
            </ul>
          </div>
        ))}
      </Container>
      <Container className="flex flex-col gap-2 border-t border-line py-6 text-xs text-muted sm:flex-row sm:justify-between">
        <p>© {SITE.company}. Orbyn Forge is open source under the Apache License 2.0.</p>
        <p>Made in Lahore.</p>
      </Container>
    </footer>
  );
}

export function Layout({ path, children }: { path: string; children: ReactNode }) {
  return (
    <div className="flex min-h-screen flex-col">
      <a href="#main" className="sr-only focus:not-sr-only focus:fixed focus:top-2 focus:left-2 focus:z-50 focus:rounded-md focus:bg-raised focus:px-3 focus:py-2">
        Skip to content
      </a>
      <Header path={path} />
      <main id="main" className="flex-1">{children}</main>
      <Footer />
    </div>
  );
}
