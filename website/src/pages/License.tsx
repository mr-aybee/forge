import { Check, Info } from "lucide-react";
// One source of truth: the same file ships in every release package.
import licenseText from "../../../LICENSE?raw";
import { Container, PageHero } from "../components/ui";

const CAN = [
  "Use Forge for any purpose, including commercial work",
  "Read, modify and build the source code",
  "Distribute original or modified copies, in source or binary form",
  "Use the patent rights contributors grant under the license",
];
const MUST = [
  "Include a copy of the license and keep copyright notices",
  "State the changes you made in modified files",
  "Not use the Orbyn names or logos to endorse your version",
  "Accept that Forge comes without warranty",
];

export function License() {
  return (
    <>
      <PageHero
        eyebrow="License"
        title="Open source. For everyone."
        lead="Orbyn Forge is open source under the Apache License 2.0. The source is on GitHub at orbyntech/forge."
      />
      <Container className="grid gap-4 py-16 md:grid-cols-2">
        <div className="rounded-[var(--forge-radius)] border border-line bg-raised p-6">
          <h2 className="font-semibold">You can</h2>
          <ul className="mt-4 space-y-3">
            {CAN.map((c) => (
              <li key={c} className="flex gap-3 text-sm">
                <Check className="mt-0.5 size-4 shrink-0 text-ok" aria-hidden="true" /> {c}
              </li>
            ))}
          </ul>
        </div>
        <div className="rounded-[var(--forge-radius)] border border-line bg-raised p-6">
          <h2 className="font-semibold">You must</h2>
          <ul className="mt-4 space-y-3">
            {MUST.map((c) => (
              <li key={c} className="flex gap-3 text-sm">
                <Info className="mt-0.5 size-4 shrink-0 text-muted" aria-hidden="true" /> {c}
              </li>
            ))}
          </ul>
        </div>
        <p className="text-sm text-muted md:col-span-2">This summary is for convenience. The full license below is what applies.</p>
      </Container>
      <Container className="pb-20">
        <h2 className="mb-4 text-2xl font-semibold tracking-tight">Full license</h2>
        <pre className="overflow-x-auto rounded-[var(--forge-radius)] border border-line bg-sunken p-6 font-mono text-[13px] leading-relaxed whitespace-pre-wrap text-fg">
          {licenseText}
        </pre>
      </Container>
    </>
  );
}
