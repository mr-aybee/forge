import type { SecretFileRef, ServiceInput } from "../../api/types";
import { useVaultItems } from "../../api/vault";
import { errorText } from "../../app/toast";
import { Input, Mono } from "../ui";
import { RowsEditor, SubBlock } from "./parts";
import { defaultTarget, targetOk } from "./sections";

const octal = (m: number | undefined) => (m === undefined ? "" : m.toString(8).padStart(4, "0"));

/**
 * Secret and config files (D-179): each row names a credential vault item
 * and where it appears. Only names travel; the values stay sealed on the
 * server until a deploy writes them on the node.
 */
export function FilesSection({ value, set }: { value: ServiceInput; set: (p: Partial<ServiceInput>) => void }) {
  const items = useVaultItems();
  const names = (items.data ?? []).map((i) => i.name);
  const native = value.runtime === "systemd";
  const listId = "secret-file-vault-items";

  return (
    <div className="space-y-3">
      {items.isError && (
        <p className="text-xs text-warn" role="status">
          The vault list is not available to you ({errorText(items.error)}). Mounting a vault item needs a platform administrator with MFA; you can still keep the files already set.
        </p>
      )}
      <datalist id={listId}>
        {names.map((n) => (
          <option key={n} value={n} />
        ))}
      </datalist>
      <SubBlock
        title="Files"
        hint={
          native ? (
            <>
              Written to a root-owned folder readable by the service&apos;s account; its path is in <Mono>FORGE_SECRETS_DIR</Mono>. Each file is named after the last part of its target.
            </>
          ) : (
            <>
              Mounted read-only. Secrets default to <Mono>/run/secrets/&lt;name&gt;</Mono> with mode 0400, configs to mode 0444. A non-root container needs the owner (uid) or a mode its user can read.
            </>
          )
        }
      >
        <RowsEditor<SecretFileRef>
          label="File"
          rows={value.secret_files}
          onChange={(secret_files) => set({ secret_files })}
          blank={() => ({ kind: "secret", item: "", target: "" })}
          addLabel="Add file"
          max={50}
          render={(f, setRow, i) => {
            const targetBad = !!f.target && !targetOk(f.target);
            return (
              <>
                <select aria-label={`File ${i + 1} kind`} className="h-7 shrink-0 rounded border border-line bg-surface px-1 text-xs" value={f.kind} onChange={(e) => setRow({ ...f, kind: e.target.value === "config" ? "config" : "secret" })}>
                  <option value="secret">Secret</option>
                  <option value="config">Config</option>
                </select>
                <Input
                  aria-label={`File ${i + 1} vault item`}
                  list={listId}
                  className="h-7 min-w-0 flex-1 py-0 text-xs"
                  value={f.item}
                  placeholder="Vault item name"
                  onChange={(e) => setRow({ ...f, item: e.target.value, target: f.target || (e.target.value ? defaultTarget(f.kind, e.target.value) : "") })}
                />
                <Input aria-label={`File ${i + 1} target`} aria-invalid={targetBad || undefined} className={`h-7 min-w-0 flex-1 py-0 font-mono text-xs ${targetBad ? "border-bad" : ""}`} value={f.target} placeholder={f.kind === "secret" ? "/run/secrets/db_password" : "/etc/app/app.conf"} onChange={(e) => setRow({ ...f, target: e.target.value.trim() })} spellCheck={false} />
                <Input
                  aria-label={`File ${i + 1} mode`}
                  className="h-7 w-16 shrink-0 py-0 font-mono text-xs"
                  value={octal(f.mode)}
                  placeholder={f.kind === "secret" ? "0400" : "0444"}
                  maxLength={4}
                  onChange={(e) => {
                    const t = e.target.value.trim();
                    const { mode: _drop, ...rest } = f;
                    setRow(t === "" || !/^[0-7]{1,4}$/.test(t) ? rest : { ...rest, mode: parseInt(t, 8) });
                  }}
                  spellCheck={false}
                />
                {!native && (
                  <Input
                    aria-label={`File ${i + 1} owner uid`}
                    className="h-7 w-16 shrink-0 py-0 font-mono text-xs"
                    value={f.uid === undefined ? "" : String(f.uid)}
                    placeholder="uid 0"
                    onChange={(e) => {
                      const t = e.target.value.trim();
                      const { uid: _drop, ...rest } = f;
                      setRow(/^\d{1,9}$/.test(t) ? { ...rest, uid: Number(t) } : rest);
                    }}
                    spellCheck={false}
                  />
                )}
              </>
            );
          }}
        />
      </SubBlock>
    </div>
  );
}
