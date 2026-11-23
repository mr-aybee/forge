import { Plus, Trash2 } from "lucide-react";

import type { CatalogPackage, ServiceDependency } from "../../api/types";
import { Button, IconButton, Select } from "../ui";

/** Language runtimes from the App Store, installed on each node before a deploy. */
export function RuntimeSection({ value, onChange, runtimes, loading }: { value: ServiceDependency[]; onChange: (v: ServiceDependency[]) => void; runtimes: CatalogPackage[]; loading: boolean }) {
  const add = () => {
    const free = runtimes.find((r) => !value.some((d) => d.package === r.id));
    if (free) onChange([...value, { package: free.id, version: free.versions[0] ?? "", params: Object.fromEntries(free.params.map((p) => [p.key, p.default])) }]);
  };
  return (
    <div className="space-y-2">
      {value.length === 0 ? (
        <p className="rounded-md border border-dashed border-line px-3 py-2 text-xs text-muted">
          <b className="text-fg">None: self-contained.</b> The service needs nothing installed, for example a Go binary or a self-contained .NET publish.
        </p>
      ) : (
        <div className="overflow-x-auto rounded-md border border-line">
          <table className="w-full min-w-[520px] text-[13px]">
            <thead className="bg-panel-2 text-left text-[11px] font-medium tracking-wide text-faint uppercase">
              <tr>
                <th className="px-2 py-1 font-medium">Runtime</th>
                <th className="w-40 px-2 py-1 font-medium">Version</th>
                <th className="px-2 py-1 font-medium">Options</th>
                <th className="w-10" />
              </tr>
            </thead>
            <tbody className="divide-y divide-line">
              {value.map((d, i) => {
                const pkg = runtimes.find((r) => r.id === d.package);
                const set = (p: Partial<ServiceDependency>) => onChange(value.map((x, j) => (j === i ? { ...x, ...p } : x)));
                const choices = (pkg?.params ?? []).filter((p) => p.kind.type === "choice");
                return (
                  <tr key={d.package}>
                    <td className="px-2 py-1">
                      <Select
                        aria-label={`Runtime ${i + 1}`}
                        value={d.package}
                        onChange={(e) => {
                          const p = runtimes.find((r) => r.id === e.target.value);
                          if (p) set({ package: p.id, version: p.versions[0] ?? "", params: Object.fromEntries(p.params.map((x) => [x.key, x.default])) });
                        }}
                      >
                        {!pkg && <option value={d.package}>{d.package}</option>}
                        {runtimes.map((r) => (
                          <option key={r.id} value={r.id} disabled={r.id !== d.package && value.some((x) => x.package === r.id)}>
                            {r.name}
                          </option>
                        ))}
                      </Select>
                    </td>
                    <td className="px-2 py-1">
                      <Select aria-label={`Version of ${pkg?.name ?? d.package}`} value={d.version} onChange={(e) => set({ version: e.target.value })}>
                        {!(pkg?.versions ?? []).includes(d.version) && <option value={d.version}>{d.version || "—"}</option>}
                        {(pkg?.versions ?? []).map((v) => (
                          <option key={v} value={v}>
                            {v}
                          </option>
                        ))}
                      </Select>
                    </td>
                    <td className="px-2 py-1">
                      {choices.length === 0 ? (
                        <span className="text-xs text-faint">—</span>
                      ) : (
                        <div className="flex flex-wrap gap-2">
                          {choices.map((p) => (
                            <Select key={p.key} aria-label={p.label} value={d.params[p.key] ?? p.default} onChange={(e) => set({ params: { ...d.params, [p.key]: e.target.value } })}>
                              {(p.kind.type === "choice" ? (p.kind as { options: string[] }).options : []).map((o) => (
                                <option key={o} value={o}>
                                  {p.label}: {o}
                                </option>
                              ))}
                            </Select>
                          ))}
                        </div>
                      )}
                    </td>
                    <td className="px-1 py-1 text-right">
                      <IconButton type="button" label={`Remove ${pkg?.name ?? d.package}`} className="size-7 hover:text-bad" onClick={() => onChange(value.filter((_, j) => j !== i))}>
                        <Trash2 className="size-3.5" />
                      </IconButton>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
      <Button type="button" variant="ghost" onClick={add} disabled={loading || value.length >= runtimes.length} title={loading ? "Loading the App Store…" : runtimes.length === 0 ? "The App Store has no runtimes" : undefined}>
        <Plus className="size-4" /> Add a runtime
      </Button>
    </div>
  );
}
