import { Boxes, Download, Eraser, History, Search, Tag, Trash2, Upload } from "lucide-react";
import { Link } from "@tanstack/react-router";
import { useMemo, useState } from "react";

import { type DockerImage, type DockerInspectOp, useDockerImages } from "../../api/dockerResources";
import { Badge, Button, Confirm, CopyIcon, Empty, Field, Input, Modal, RowMenu, type RowAction, Segmented, Select, Skeleton, Switch, Table, Td, ago, bytes, time, useRowMenu } from "../ui";
import { Check, DockerProblem, type DockerRunner, InspectDrawer, Spin, Toolbar, opKey } from "./shared";

type Filter = "all" | "dangling" | "unused";
const label = (i: DockerImage) => i.tags[0] ?? `<none> ${i.id}`;
/** Docker reports -1 when it did not count; treat that as unknown. */
const inUse = (i: DockerImage) => (i.containers ?? -1) > 0;
const unused = (i: DockerImage) => i.containers === 0;

export function DockerImages({ server, runner }: { server: string; runner: DockerRunner }) {
  const q = useDockerImages(server);
  const [search, setSearch] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const [picked, setPicked] = useState<Set<string>>(() => new Set());
  const [dialog, setDialog] = useState<
    | { kind: "pull" }
    | { kind: "prune" }
    | { kind: "tag"; image: DockerImage }
    | { kind: "push"; image: DockerImage }
    | { kind: "remove"; image: DockerImage }
    | { kind: "bulk" }
    | { kind: "inspect"; op: DockerInspectOp; title: string }
    | null
  >(null);
  const rm = useRowMenu();
  const all = useMemo(() => q.data ?? [], [q.data]);
  const rows = useMemo(() => {
    const s = search.trim().toLowerCase();
    return all.filter(
      (i) =>
        (filter === "all" || (filter === "dangling" ? i.dangling : unused(i))) &&
        (!s || `${i.id} ${i.tags.join(" ")} ${i.digests.join(" ")}`.toLowerCase().includes(s)),
    );
  }, [all, search, filter]);
  const sel = rows.filter((i) => picked.has(i.full_id));
  const selectable = rows.filter((i) => !inUse(i));
  const flip = (id: string, on: boolean) =>
    setPicked((p) => {
      const n = new Set(p);
      if (on) n.add(id);
      else n.delete(id);
      return n;
    });
  const total = all.reduce((a, i) => a + (i.size ?? 0), 0);
  const close = () => setDialog(null);

  return (
    <div>
      <Toolbar>
        <label className="flex min-w-0 flex-1 items-center gap-2">
          <Search className="size-3.5 shrink-0 text-faint" />
          <input data-bare className="min-w-0 flex-1 bg-transparent text-[13px] outline-none" placeholder="Filter by tag, id or digest" aria-label="Filter images" value={search} onChange={(e) => setSearch(e.target.value)} />
        </label>
        <Segmented
          label="Image filter"
          value={filter}
          onChange={setFilter}
          options={[
            { id: "all", label: "All", count: all.length },
            { id: "dangling", label: "Dangling", count: all.filter((i) => i.dangling).length, title: "Untagged layers left behind by rebuilds and pulls" },
            { id: "unused", label: "Unused", count: all.filter(unused).length, title: "No container uses it" },
          ]}
        />
        {sel.length > 0 && (
          <Button variant="danger" disabled={runner.bulk} onClick={() => setDialog({ kind: "bulk" })}>
            <Spin on={runner.bulk}>
              <Trash2 className="size-4" />
            </Spin>{" "}
            Remove {sel.length}
          </Button>
        )}
        <Button onClick={() => setDialog({ kind: "prune" })}>
          <Eraser className="size-4" /> Prune
        </Button>
        <Button variant="primary" onClick={() => setDialog({ kind: "pull" })}>
          <Download className="size-4" /> Pull image
        </Button>
      </Toolbar>
      {q.error ? (
        <DockerProblem error={q.error} server={server} />
      ) : q.isLoading ? (
        <Skeleton rows={4} />
      ) : rows.length === 0 ? (
        <Empty icon={<Boxes className="size-5" />} title={all.length ? "No image matches" : "No images"}>
          {all.length ? "Change the filter or the search." : "Pull an image, or create a container; Docker pulls its image."}
        </Empty>
      ) : (
        <>
          <Table
            head={[
              <Check
                key="all"
                label="Select every removable image shown"
                checked={selectable.length > 0 && selectable.every((i) => picked.has(i.full_id))}
                disabled={selectable.length === 0}
                onChange={(on) => setPicked(on ? new Set(selectable.map((i) => i.full_id)) : new Set())}
              />,
              "Image",
              "Id",
              "Size",
              "Created",
              "Used by",
              "",
            ]}
          >
            {rows.map((i) => {
              const busyRm = runner.busy === opKey({ op: "image_remove", id: i.full_id, force: false });
              const actions: RowAction[] = [
                { label: "Inspect", icon: Search, tone: "accent", onSelect: () => setDialog({ kind: "inspect", op: { op: "image_inspect", id: i.full_id }, title: `Inspect · ${label(i)}` }) },
                { label: "History", icon: History, tone: "accent", onSelect: () => setDialog({ kind: "inspect", op: { op: "image_history", id: i.full_id }, title: `History · ${label(i)}` }) },
                { label: "Tag…", icon: Tag, onSelect: () => setDialog({ kind: "tag", image: i }) },
                { label: "Push…", icon: Upload, disabled: i.tags.length === 0, hint: "Tag the image first", onSelect: () => setDialog({ kind: "push", image: i }) },
                { label: "Remove…", icon: Trash2, danger: true, disabled: runner.pending, hint: "Another Docker action is running", onSelect: () => setDialog({ kind: "remove", image: i }) },
              ];
              return (
                <tr key={i.full_id} className="hover:bg-panel-2/60" {...rm.rowProps(actions, `Actions for ${label(i)}`)}>
                  <Td className="w-8">
                    <Check
                      label={`Select ${label(i)}`}
                      checked={picked.has(i.full_id)}
                      disabled={inUse(i)}
                      title={inUse(i) ? "A container uses this image" : undefined}
                      onChange={(on) => flip(i.full_id, on)}
                    />
                  </Td>
                  <Td className="max-w-[360px]">
                    <div className="flex min-w-0 flex-wrap items-center gap-1">
                      {i.tags.length ? (
                        i.tags.map((t) => (
                          <span key={t} className="max-w-full truncate font-mono text-[12px]" title={t}>
                            {t}
                          </span>
                        ))
                      ) : (
                        <span className="font-mono text-[12px] text-faint">&lt;none&gt;</span>
                      )}
                      {i.dangling && <Badge tone="warn">dangling</Badge>}
                    </div>
                    {i.digests[0] && (
                      <div className="truncate font-mono text-[10px] text-faint" title={i.digests.join("\n")}>
                        {i.digests[0]}
                      </div>
                    )}
                  </Td>
                  <Td className="whitespace-nowrap">
                    <span className="font-mono text-[11px] text-muted tabular-nums select-all" title={i.full_id}>
                      {i.id}
                    </span>
                    <CopyIcon text={i.full_id} label="Copy image id" />
                  </Td>
                  <Td className="font-mono text-[11px] whitespace-nowrap text-muted tabular-nums">{bytes(i.size)}</Td>
                  <Td className="text-xs whitespace-nowrap text-muted" title={time(i.created)}>
                    {i.created ? ago(i.created) : "—"}
                  </Td>
                  <Td className="font-mono text-[11px] text-muted tabular-nums">{i.containers === undefined || i.containers < 0 ? "—" : i.containers}</Td>
                  <Td className="text-right">
                    {busyRm ? <Spin on>{null}</Spin> : <RowMenu actions={actions} label={`Actions for ${label(i)}`} />}
                  </Td>
                </tr>
              );
            })}
          </Table>
          <p className="border-t border-line px-4 py-1.5 font-mono text-[11px] text-faint tabular-nums">
            {rows.length} of {all.length} images · {bytes(total)} total (shared layers counted once per image)
          </p>
        </>
      )}
      {rm.menu}

      {dialog?.kind === "inspect" && <InspectDrawer server={server} op={dialog.op} title={dialog.title} onClose={close} />}
      {dialog?.kind === "pull" && <PullDialog runner={runner} onClose={close} />}
      {dialog?.kind === "push" && <PushDialog image={dialog.image} runner={runner} onClose={close} />}
      {dialog?.kind === "tag" && <TagDialog image={dialog.image} runner={runner} onClose={close} />}
      {dialog?.kind === "remove" && <RemoveImage image={dialog.image} runner={runner} onClose={close} />}
      {dialog?.kind === "prune" && <PruneImages runner={runner} onClose={close} />}
      {dialog?.kind === "bulk" && (
        <Confirm
          open
          onOpenChange={(o) => !o && close()}
          title={`Remove ${sel.length} images?`}
          body={
            <>
              <p>Docker deletes these images from the node. Tags shared with other images are untagged only. A container that needs one later pulls it again.</p>
              <ul className="mt-2 max-h-40 overflow-auto font-mono text-[11px]">
                {sel.map((i) => (
                  <li key={i.full_id} className="truncate" title={label(i)}>
                    {label(i)}
                  </li>
                ))}
              </ul>
            </>
          }
          confirmLabel={`Remove ${sel.length}`}
          onConfirm={() => {
            const ops = sel.map((i) => ({ op: "image_remove" as const, id: i.full_id, force: false }));
            close();
            void runner.runMany(ops, "images", () => setPicked(new Set()));
          }}
        />
      )}
    </div>
  );
}

const IMAGE_REF = "[A-Za-z0-9][A-Za-z0-9/:._@\\-]*";

function PullDialog({ runner, onClose }: { runner: DockerRunner; onClose: () => void }) {
  const [image, setImage] = useState("");
  const busy = runner.pending;
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title="Pull image" description="Docker downloads the image on this node. The pull runs as a job you can follow.">
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          void runner.run({ op: "image_pull", image: image.trim() }, `Pull ${image.trim()}`).then((ok) => ok && onClose());
        }}
      >
        <Field label="Image" hint="repository[:tag] or repository@digest; :latest when no tag is given. Private registries use the credentials added in Forge.">
          <Input autoFocus required pattern={IMAGE_REF} value={image} onChange={(e) => setImage(e.target.value)} placeholder="nginx:1.27 or ghcr.io/org/app:tag" className="font-mono" />
        </Field>
        <div className="flex justify-end gap-2">
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={busy || !image.trim()}>
            <Spin on={busy}>
              <Download className="size-4" />
            </Spin>{" "}
            Pull
          </Button>
        </div>
      </form>
    </Modal>
  );
}

function PushDialog({ image, runner, onClose }: { image: DockerImage; runner: DockerRunner; onClose: () => void }) {
  const [ref, setRef] = useState(image.tags[0] ?? "");
  const [error, setError] = useState<string>();
  const busy = runner.pending;
  const host = registryHost(ref);
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title="Push image" description="Docker uploads the tag to its registry. The push runs as a job.">
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          setError(undefined);
          void runner.runPush(ref).then((r) => {
            if (r === true) onClose();
            else if (r) setError(r);
          });
        }}
      >
        <Field label="Tag to push" hint={<>Registry: <span className="font-mono">{host}</span></>}>
          <Select value={ref} onChange={(e) => setRef(e.target.value)}>
            {image.tags.map((t) => (
              <option key={t} value={t}>
                {t}
              </option>
            ))}
          </Select>
        </Field>
        <p className="text-xs text-muted">
          Forge pushes only to registries added under{" "}
          <Link to="/settings/registries" className="text-accent hover:underline">
            Settings → Registries
          </Link>
          , using the credentials stored there. Tag the image with that registry's host first (for example <span className="font-mono">ghcr.io/org/app:1.0</span>).
        </p>
        {error && <p className="rounded-md bg-bad-soft px-3 py-2 text-xs text-bad">{error}</p>}
        <div className="flex justify-end gap-2">
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={busy || !ref}>
            <Spin on={busy}>
              <Upload className="size-4" />
            </Spin>{" "}
            Push
          </Button>
        </div>
      </form>
    </Modal>
  );
}

/** The registry host of an image reference, as Docker reads it. */
function registryHost(ref: string): string {
  const first = ref.split("/")[0] ?? "";
  return ref.includes("/") && (first.includes(".") || first.includes(":") || first === "localhost") ? first : "docker.io";
}

function TagDialog({ image, runner, onClose }: { image: DockerImage; runner: DockerRunner; onClose: () => void }) {
  const first = image.tags[0];
  const cut = first ? first.lastIndexOf(":") : -1;
  const [form, setForm] = useState({ repo: cut > 0 ? first!.slice(0, cut) : "", tag: "" });
  const busy = runner.pending;
  return (
    <Modal open onOpenChange={(o) => !o && onClose()} title="Tag image" description={`Adds a name to ${image.tags[0] ?? image.id}. The image itself does not change.`}>
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          const repo = form.repo.trim();
          const tag = form.tag.trim() || "latest";
          void runner.run({ op: "image_tag", id: image.full_id, repo, tag }, `Tagged ${repo}:${tag}`).then((ok) => ok && onClose());
        }}
      >
        <div className="grid grid-cols-[1fr_140px] gap-2">
          <Field label="Repository">
            <Input autoFocus required pattern="[a-z0-9][a-z0-9/:._\-]*" className="font-mono" value={form.repo} onChange={(e) => setForm({ ...form, repo: e.target.value })} placeholder="registry.example.com/team/app" />
          </Field>
          <Field label="Tag" hint="Default latest">
            <Input pattern="[A-Za-z0-9_][A-Za-z0-9_.\-]{0,127}" className="font-mono" value={form.tag} onChange={(e) => setForm({ ...form, tag: e.target.value })} placeholder="latest" />
          </Field>
        </div>
        <div className="flex justify-end gap-2">
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={busy || !form.repo.trim()}>
            <Spin on={busy}>
              <Tag className="size-4" />
            </Spin>{" "}
            Tag
          </Button>
        </div>
      </form>
    </Modal>
  );
}

function RemoveImage({ image, runner, onClose }: { image: DockerImage; runner: DockerRunner; onClose: () => void }) {
  const [force, setForce] = useState(false);
  const used = inUse(image);
  return (
    <Confirm
      open
      onOpenChange={(o) => !o && onClose()}
      title={`Remove ${label(image)}?`}
      busy={runner.pending}
      body={
        <div className="space-y-3">
          <p>
            Docker deletes the image from this node.{" "}
            {used ? `${image.containers} container${image.containers === 1 ? "" : "s"} use it; Docker refuses unless you force it, and they keep running on the removed image.` : "No container uses it."}
          </p>
          <Switch checked={force} onChange={setForce} label="Force: remove even when containers use it or other tags point to it" />
        </div>
      }
      confirmLabel={force ? "Force remove" : "Remove"}
      onConfirm={() => void runner.run({ op: "image_remove", id: image.full_id, force }, `Removed ${label(image)}`).then((ok) => ok && onClose())}
    />
  );
}

function PruneImages({ runner, onClose }: { runner: DockerRunner; onClose: () => void }) {
  const [all, setAll] = useState(false);
  return (
    <Confirm
      open
      onOpenChange={(o) => !o && onClose()}
      title="Prune images?"
      busy={runner.pending}
      body={
        <div className="space-y-3">
          <p>What goes:</p>
          <ul className="list-disc space-y-1 pl-5">
            <li>Dangling images: untagged layers no container uses.</li>
            {all && <li>Every image no container uses, tagged or not. Containers created later pull them again.</li>}
          </ul>
          <p>Images used by any container, running or stopped, stay. The prune runs as a job.</p>
          <p className="text-warn">This is host-wide: it covers every unused image on this node, including ones Forge did not pull.</p>
          <Switch checked={all} onChange={setAll} label="Also remove unused tagged images" />
        </div>
      }
      confirmLabel={all ? "Prune all unused" : "Prune dangling"}
      onConfirm={() => void runner.run({ op: "image_prune", all }, "Image prune").then((ok) => ok && onClose())}
    />
  );
}
