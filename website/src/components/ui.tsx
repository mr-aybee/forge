import { useState, type ReactNode } from "react";
import { Check, Copy } from "lucide-react";

type ButtonProps = {
  href: string;
  children: ReactNode;
  variant?: "primary" | "secondary" | "ghost";
  external?: boolean;
};

const VARIANTS = {
  primary:
    "bg-accent text-on-accent hover:bg-accent-strong shadow-[0_10px_30px_-12px_var(--forge-accent)] hover:-translate-y-px",
  secondary: "border border-line bg-raised text-fg shadow-card hover:border-accent",
  ghost: "text-fg hover:text-accent",
};

export function Button({ href, children, variant = "primary", external }: ButtonProps) {
  return (
    <a
      href={href}
      {...(external ? { target: "_blank", rel: "noopener noreferrer" } : {})}
      className={`inline-flex h-11 items-center justify-center gap-2 rounded-full px-5 text-sm font-semibold transition-colors ${VARIANTS[variant]}`}
    >
      {children}
    </a>
  );
}

export function Container({ children, className = "" }: { children: ReactNode; className?: string }) {
  return <div className={`mx-auto w-full max-w-6xl px-4 sm:px-6 ${className}`}>{children}</div>;
}

export function Eyebrow({ children }: { children: ReactNode }) {
  return (
    <p className="mb-3 text-xs font-semibold uppercase tracking-[0.16em] text-accent">{children}</p>
  );
}

export function SectionHeading({
  eyebrow,
  title,
  lead,
  center,
}: {
  eyebrow?: string;
  title: ReactNode;
  lead?: ReactNode;
  center?: boolean;
}) {
  return (
    <div className={`mb-12 max-w-2xl ${center ? "mx-auto text-center" : ""}`}>
      {eyebrow && <Eyebrow>{eyebrow}</Eyebrow>}
      <h2 className="text-3xl font-semibold tracking-tight text-balance sm:text-4xl">{title}</h2>
      {lead && <p className="mt-4 text-lg text-pretty text-muted">{lead}</p>}
    </div>
  );
}

export function PageHero({ eyebrow, title, lead, children }: { eyebrow: string; title: ReactNode; lead: ReactNode; children?: ReactNode }) {
  return (
    <section className="relative overflow-hidden border-b border-line">
      <div className="aurora pointer-events-none absolute inset-0" />
      <div className="grid-bg pointer-events-none absolute inset-0 opacity-60" />
      <Container className="relative py-20 sm:py-24">
        <Eyebrow>{eyebrow}</Eyebrow>
        <h1 className="max-w-3xl text-4xl font-semibold tracking-tight text-balance sm:text-5xl">{title}</h1>
        <p className="mt-5 max-w-2xl text-lg text-pretty text-muted">{lead}</p>
        {children && <div className="mt-8 flex flex-wrap gap-3">{children}</div>}
      </Container>
    </section>
  );
}

/** A terminal-styled code block with a copy button. */
export function CodeBlock({ code, label }: { code: string; label?: string }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      // Clipboard can be blocked (insecure context, permissions); the text
      // stays selectable, so there is nothing else to do.
    }
  };
  return (
    <div className="overflow-hidden rounded-xl border border-line bg-[#06111a] text-[#d6e6ef]">
      <div className="flex items-center justify-between border-b border-white/10 px-4 py-2">
        <div className="flex items-center gap-1.5" aria-hidden="true">
          <span className="size-2.5 rounded-full bg-white/15" />
          <span className="size-2.5 rounded-full bg-white/15" />
          <span className="size-2.5 rounded-full bg-white/15" />
          {label && <span className="ml-3 text-xs text-white/50">{label}</span>}
        </div>
        <button
          type="button"
          onClick={copy}
          className="inline-flex items-center gap-1.5 rounded-md px-2 py-1 text-xs text-white/60 hover:bg-white/10 hover:text-white"
          aria-label="Copy to clipboard"
        >
          {copied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
          {copied ? "Copied" : "Copy"}
        </button>
      </div>
      <pre className="overflow-x-auto p-4 font-mono text-[13px] leading-relaxed">
        <code>{code}</code>
      </pre>
    </div>
  );
}
