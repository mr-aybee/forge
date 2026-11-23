import { ArrowRight } from "lucide-react";
import { FEATURE_GROUPS, FEATURES, docs } from "../content/site";
import { Container, PageHero } from "../components/ui";
import { IconTile } from "../components/Icon";
import { CallToAction, slug } from "./Home";

export function Features() {
  return (
    <>
      <PageHero
        eyebrow="Features"
        title="Everything Forge does, in one place."
        lead="Forge drives the engines you already trust (Docker, Nginx, PowerDNS, database servers) and adds one permission model, one audit trail and one console on top."
      />
      <div className="border-b border-line bg-sunken">
        <Container className="flex gap-2 overflow-x-auto py-3">
          {FEATURE_GROUPS.map((g) => (
            <a key={g.id} href={`#${slug(g.id)}`} className="shrink-0 rounded-full border border-line bg-raised px-3.5 py-1.5 text-sm text-muted hover:border-accent hover:text-fg">
              {g.id}
            </a>
          ))}
        </Container>
      </div>
      {FEATURE_GROUPS.map((g) => (
        <section key={g.id} id={slug(g.id)} className="scroll-mt-20 border-b border-line py-16">
          <Container className="grid gap-10 lg:grid-cols-[260px_1fr]">
            <div>
              <h2 className="text-2xl font-semibold tracking-tight">{g.id}</h2>
              <p className="mt-2 text-muted">{g.blurb}</p>
            </div>
            <div className="grid gap-4 sm:grid-cols-2">
              {FEATURES.filter((f) => f.group === g.id).map((f) => (
                <a key={f.title} href={docs(f.doc)} className="group rounded-[var(--forge-radius)] border border-line bg-raised p-6 hover:border-accent">
                  <IconTile name={f.icon} />
                  <h3 className="mt-4 font-semibold">{f.title}</h3>
                  <p className="mt-2 text-sm text-pretty text-muted">{f.body}</p>
                  <span className="mt-4 inline-flex items-center gap-1 text-sm font-medium text-accent">
                    Docs <ArrowRight className="size-3.5 transition-transform group-hover:translate-x-0.5" aria-hidden="true" />
                  </span>
                </a>
              ))}
            </div>
          </Container>
        </section>
      ))}
      <CallToAction />
    </>
  );
}
