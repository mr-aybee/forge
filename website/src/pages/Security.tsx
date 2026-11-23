import { ArrowRight, Mail } from "lucide-react";
import { SECURITY_POINTS, docs } from "../content/site";
import { Button, Container, PageHero, SectionHeading } from "../components/ui";
import { IconTile } from "../components/Icon";

const LAYERS = ["API handler", "MCP endpoint", "Job runner (every step)", "Agent hub", "Agent on the node", "Plugin host"];

export function Security() {
  return (
    <>
      <PageHero
        eyebrow="Security"
        title="Built so that one mistake is not a breach."
        lead="Forge controls machines, so it is designed around least privilege, short-lived credentials and a record you can prove. Here is what it enforces."
      >
        <Button href={docs("security/")}>
          Security overview <ArrowRight className="size-4" aria-hidden="true" />
        </Button>
        <Button href={docs("security/hardening/")} variant="secondary">
          Hardening guide
        </Button>
      </PageHero>

      <section className="py-20">
        <Container>
          <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
            {SECURITY_POINTS.map((p) => (
              <div key={p.title} className="rounded-[var(--forge-radius)] border border-line bg-raised p-6">
                <IconTile name={p.icon} />
                <h3 className="mt-4 font-semibold">{p.title}</h3>
                <p className="mt-2 text-sm text-pretty text-muted">{p.body}</p>
              </div>
            ))}
          </div>
        </Container>
      </section>

      <section className="border-y border-line bg-sunken py-20">
        <Container className="grid items-center gap-12 lg:grid-cols-2">
          <SectionHeading
            eyebrow="Defence in depth"
            title="Authorized at every layer, not just the UI."
            lead="A request that passes the API is checked again by the job runner before each step, by the agent hub, and by the agent on the node. A bug in one layer does not open the others."
          />
          <ol className="space-y-2">
            {LAYERS.map((l, i) => (
              <li
                key={l}
                className="flex items-center gap-4 rounded-xl border border-line bg-raised px-4 py-3"
                style={{ marginLeft: `${i * 12}px` }}
              >
                <span className="font-mono text-xs text-accent">{String(i + 1).padStart(2, "0")}</span>
                <span className="text-sm font-medium">{l}</span>
                <span className="ml-auto rounded-full bg-accent-soft px-2 py-0.5 font-mono text-[11px] text-accent">cedar</span>
              </li>
            ))}
          </ol>
        </Container>
      </section>

      <section className="py-20">
        <Container className="max-w-3xl text-center">
          <h2 className="text-2xl font-semibold tracking-tight">Report a vulnerability</h2>
          <p className="mt-3 text-muted">
            Please report security issues privately through GitHub Security Advisories, not in a public issue.
          </p>
          <div className="mt-6 flex justify-center">
            <Button href="https://github.com/mr-aybee/forge/security/advisories/new" variant="secondary">
              <Mail className="size-4" aria-hidden="true" /> Report a vulnerability
            </Button>
          </div>
        </Container>
      </section>
    </>
  );
}
