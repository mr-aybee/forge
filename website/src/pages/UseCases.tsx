import { ArrowRight, Check } from "lucide-react";
import { USE_CASES, docs } from "../content/site";
import { Container, PageHero } from "../components/ui";
import { IconTile } from "../components/Icon";
import { CallToAction, slug } from "./Home";

export function UseCases() {
  return (
    <>
      <PageHero
        eyebrow="Use cases"
        title="One platform, many ways to put it to work."
        lead="From a handful of servers to a thousand nodes across clients and clouds, here is how teams use Forge, and where to start in the docs."
      />
      <Container className="space-y-6 py-16">
        {USE_CASES.map((u) => (
          <article
            key={u.title}
            id={slug(u.title)}
            className="scroll-mt-24 grid gap-8 rounded-[var(--forge-radius)] border border-line bg-raised p-6 sm:p-8 lg:grid-cols-[1fr_1.2fr]"
          >
            <div>
              <div className="flex items-center gap-3">
                <IconTile name={u.icon} />
                <p className="text-xs font-medium uppercase tracking-wider text-muted">{u.who}</p>
              </div>
              <h2 className="mt-5 text-2xl font-semibold tracking-tight text-balance">{u.title}</h2>
              <p className="mt-3 text-pretty text-muted">{u.problem}</p>
            </div>
            <div className="rounded-xl bg-sunken p-6">
              <p className="mb-4 text-sm font-semibold">With Forge</p>
              <ul className="space-y-3">
                {u.solution.map((s) => (
                  <li key={s} className="flex gap-3 text-sm">
                    <Check className="mt-0.5 size-4 shrink-0 text-accent" aria-hidden="true" />
                    <span>{s}</span>
                  </li>
                ))}
              </ul>
              <div className="mt-6 flex flex-wrap gap-2">
                {u.docs.map((d) => (
                  <a key={d.path} href={docs(d.path)} className="inline-flex items-center gap-1 rounded-full border border-line bg-raised px-3 py-1 text-sm hover:border-accent hover:text-accent">
                    {d.label} <ArrowRight className="size-3" aria-hidden="true" />
                  </a>
                ))}
              </div>
            </div>
          </article>
        ))}
      </Container>
      <CallToAction />
    </>
  );
}
