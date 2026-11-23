import { type ReactNode, useState } from "react";

import { useCatalog } from "../../api/hooks";
import type { EditableRuntime, ServiceInput } from "../../api/types";
import { Confirm } from "../ui";
import { AdvancedSection, YamlToggle } from "./AdvancedSection";
import { CommandSection } from "./CommandSection";
import { EnvSection } from "./EnvSection";
import { HealthSection } from "./HealthSection";
import { ImageSection } from "./ImageSection";
import { SectionCard } from "./parts";
import { PortsSection } from "./PortsSection";
import { ResourcesSection } from "./ResourcesSection";
import { RuntimeSection } from "./RuntimeSection";
import { CLEARED_ON_SWITCH, KINDS, SECTIONS, type BasicsState, type SectionId, sectionDomId, sectionIssues, switchKind } from "./sections";
import { StartSection } from "./StartSection";
import { StatusMark } from "./StatusMark";
import { StorageSection } from "./StorageSection";
import { VolumesSection } from "./VolumesSection";

export interface ServiceEditorProps {
  value: ServiceInput;
  onChange: (v: ServiceInput) => void;
  /** On edit, changing the kind asks first. */
  mode: "create" | "edit";
  /** Body of the Basics card (create or edit variant) and the state its status comes from. */
  basics: ReactNode;
  basicsState: BasicsState;
  /** Paths and names on the node, for hints. */
  facts: { home: string; envFile: string; unit: string; user: string };
  /** Set on edit: links to the service's other tabs. */
  serviceId?: string;
}

/**
 * The service editor: two kind cards on top, then a sticky rail of the kind's
 * sections (with complete / needs input / error markers) beside wide,
 * always-open section cards. Shared by **New service** and the service's
 * **Configuration** tab; the page owns the form object and the action bar.
 */
export function ServiceEditor({ value, onChange, mode, basics, basicsState, facts, serviceId }: ServiceEditorProps) {
  const catalog = useCatalog();
  const packages = catalog.data?.packages ?? [];
  const runtimes = packages.filter((p) => p.category === "runtimes");
  const [ui, setUi] = useState<{ envMode: "grid" | "text"; yaml: boolean; pendingKind: EditableRuntime | null }>({ envMode: "grid", yaml: false, pendingKind: null });
  const set = (p: Partial<ServiceInput>) => onChange({ ...value, ...p });
  const sections = SECTIONS[value.runtime];
  const issues = Object.fromEntries(sections.map((s) => [s.id, sectionIssues(s.id, value, basicsState)])) as Record<SectionId, ReturnType<typeof sectionIssues>>;

  const pickKind = (k: EditableRuntime) => {
    if (k === value.runtime) return;
    if (mode === "edit") setUi((u) => ({ ...u, pendingKind: k }));
    else onChange(switchKind(value, k));
  };
  const jump = (id: SectionId) => {
    const el = document.getElementById(sectionDomId(id));
    el?.scrollIntoView({ behavior: "smooth", block: "start" });
    el?.querySelector<HTMLElement>("input,select,textarea,button")?.focus({ preventScroll: true });
  };

  const body = (id: SectionId): { node: ReactNode; actions?: ReactNode } => {
    switch (id) {
      case "basics":
        return { node: basics };
      case "runtime":
        return { node: <RuntimeSection value={value.dependencies} onChange={(dependencies) => set({ dependencies })} runtimes={runtimes} loading={catalog.isLoading} /> };
      case "start":
        return { node: <StartSection value={value} set={set} home={facts.home} /> };
      case "image":
        return { node: <ImageSection value={value} set={set} /> };
      case "ports":
        return { node: <PortsSection value={value} set={set} /> };
      case "command":
        return { node: <CommandSection value={value} set={set} /> };
      case "resources":
        return { node: <ResourcesSection value={value} set={set} /> };
      case "env":
        return { node: <EnvSection value={value} set={set} mode={ui.envMode} onMode={(envMode) => setUi((u) => ({ ...u, envMode }))} packages={packages} /> };
      case "storage":
        return { node: <StorageSection home={facts.home} envFile={facts.envFile} serviceId={serviceId} /> };
      case "volumes":
        return { node: <VolumesSection value={value} set={set} home={facts.home} /> };
      case "health":
        return { node: <HealthSection value={value} set={set} serviceId={serviceId} /> };
      case "advanced":
        return {
          // Keyed on the view so the YAML text starts from the current form each time it opens.
          node: <AdvancedSection key={ui.yaml ? "yaml" : "form"} value={value} set={set} onReplace={onChange} yaml={ui.yaml} facts={{ unit: facts.unit, user: facts.user }} />,
          actions: value.runtime === "container" ? <YamlToggle yaml={ui.yaml} onChange={(yaml) => setUi((u) => ({ ...u, yaml }))} /> : undefined,
        };
    }
  };

  return (
    <div className="space-y-4">
      <div role="radiogroup" aria-label="Kind" className="grid gap-3 sm:grid-cols-2">
        {KINDS.map((k) => {
          const on = value.runtime === k.id;
          return (
            <button
              key={k.id}
              type="button"
              role="radio"
              aria-checked={on}
              onClick={() => pickKind(k.id)}
              className={`press flex min-w-0 items-start gap-3 rounded-lg border p-4 text-left focus-visible:outline-2 focus-visible:outline-accent ${on ? "border-accent bg-accent-soft/60" : "border-line bg-panel hover:border-line-strong"}`}
            >
              <span className={`grid size-10 shrink-0 place-items-center rounded-md ${on ? "bg-accent text-white" : "bg-panel-2 text-muted"}`}>
                <k.icon className="size-5" aria-hidden />
              </span>
              <span className="min-w-0">
                <span className={`block text-sm font-semibold ${on ? "text-accent" : ""}`}>{k.label}</span>
                <span className="block text-xs text-muted">{k.description}</span>
                <span className="mt-1 block text-[11px] text-faint">{k.needs}</span>
              </span>
            </button>
          );
        })}
      </div>

      <div className="grid gap-4 lg:grid-cols-[200px_minmax(0,1fr)]">
        <nav aria-label="Sections" className="self-start lg:sticky lg:top-4">
          <ul className="flex gap-1 overflow-x-auto pb-1 lg:flex-col lg:overflow-visible lg:pb-0">
            {sections.map((s) => (
              <li key={s.id} className="shrink-0">
                <button
                  type="button"
                  onClick={() => jump(s.id)}
                  className="flex w-full min-w-0 items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] text-muted hover:bg-panel-2 hover:text-fg focus-visible:outline-2 focus-visible:outline-accent"
                >
                  <s.icon className="size-3.5 shrink-0" aria-hidden />
                  <span className="min-w-0 flex-1 truncate">{s.label}</span>
                  <StatusMark status={issues[s.id].status} />
                </button>
              </li>
            ))}
          </ul>
        </nav>
        <div className="min-w-0 space-y-4">
          {sections.map((s) => {
            const b = body(s.id);
            return (
              <SectionCard key={s.id} meta={s} status={issues[s.id].status} messages={issues[s.id].messages} actions={b.actions}>
                {b.node}
              </SectionCard>
            );
          })}
        </div>
      </div>

      {ui.pendingKind && (
        <Confirm
          open
          onOpenChange={(o) => !o && setUi((u) => ({ ...u, pendingKind: null }))}
          title={`Change to ${KINDS.find((k) => k.id === ui.pendingKind)?.label ?? ui.pendingKind}?`}
          confirmLabel="Change kind"
          body={
            <div className="space-y-2 text-[13px]">
              <p>This clears {CLEARED_ON_SWITCH[ui.pendingKind]}. Variables, resources and package variables stay.</p>
              <div className="rounded-md border border-line px-3 py-2 text-xs text-muted">
                <div className="mb-1 font-medium text-fg">What happens</div>
                <ul className="list-disc space-y-0.5 pl-4">
                  <li>Nothing changes on the nodes until you save and deploy.</li>
                  <li>The next deploy sets the service up again as the new kind on every node: expect a short outage.</li>
                  <li>Ingress endpoints keep their port; check that the new kind listens on it.</li>
                </ul>
              </div>
            </div>
          }
          onConfirm={() => {
            const k = ui.pendingKind;
            setUi((u) => ({ ...u, pendingKind: null }));
            if (k) onChange(switchKind(value, k));
          }}
        />
      )}
    </div>
  );
}
