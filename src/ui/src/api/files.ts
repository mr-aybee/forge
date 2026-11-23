// File explorer: roots, directories, reading, editing, uploads, trash,
// revisions and search on a node.
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";

import { Sha256 } from "../lib/sha256";
import { ApiError, api, csrfToken, get, post } from "./client";

const enc = encodeURIComponent;
const base = (serverId: string) => `/api/v1/servers/${enc(serverId)}/files`;

export type EntryKind = "file" | "dir" | "symlink" | "other";

export interface FileEntry {
  name: string;
  kind: EntryKind;
  size: number;
  modified_ms: number;
  mode?: number;
  owner?: string;
  group?: string;
  link_target?: string;
  mount: boolean;
  protected: boolean;
  readonly: boolean;
  hidden: boolean;
}

export interface Permissions {
  read: boolean;
  write: boolean;
  delete: boolean;
  admin: boolean;
}

export interface FileRoot {
  id: string;
  kind: "node" | "service" | "container";
  label: string;
  base: string;
  read_only: boolean;
  permissions: Permissions;
  container?: string;
  service_id?: string;
}

/** Roots unique by id, in server order. */
export function uniqueRoots(list: FileRoot[] | undefined): FileRoot[] {
  const seen = new Set<string>();
  return (Array.isArray(list) ? list : []).filter((r) => !!r?.id && !seen.has(r.id) && !!seen.add(r.id));
}

export interface RootsResponse {
  server_id: string;
  os: "linux" | "windows";
  mfa: boolean;
  roots: FileRoot[];
}

export interface DirPage {
  path: string;
  offset: number;
  total: number;
  truncated: boolean;
  skipped_names: number;
  entries: FileEntry[];
}

export interface ReadResult {
  path: string;
  offset: number;
  length: number;
  size: number;
  eof: boolean;
  version: string;
  modified_ms: number;
  content_b64: string;
}

export interface TrashEntry {
  id: string;
  path: string;
  deleted_at: number;
  size: number;
  kind: EntryKind;
}

export interface Revision {
  id: string;
  saved_at: number;
  size: number;
}

export interface SearchSpec {
  name?: string;
  content?: string;
  regex?: boolean;
  case_sensitive?: boolean;
  whole_word?: boolean;
  context?: number;
  include?: string[];
  exclude?: string[];
  hidden?: boolean;
  after?: string;
}

export interface SearchHit {
  path: string;
  size: number;
  modified_ms: number;
  matches?: { line: number; text: string; before?: string[]; after?: string[] }[];
}

export interface SearchResult {
  path: string;
  status: "complete" | "truncated" | "timeout";
  next?: string | null;
  files: SearchHit[];
  matches: number;
  scanned_files: number;
  scanned_bytes: number;
}

/** Bytes per read (the server's limit). */
export const READ_CHUNK = 256 * 1024;
/** Files up to this size open in the editor. */
export const EDIT_LIMIT = 2 * 1024 * 1024;
/** Files up to this size can be paged through in the viewer. */
export const VIEW_LIMIT = 50 * 1024 * 1024;
const PAGE = 500;

function normEntry(e: Partial<FileEntry> | null | undefined): FileEntry {
  const o = e ?? {};
  return {
    ...o,
    name: o.name ?? "?",
    kind: o.kind ?? "other",
    size: o.size ?? 0,
    modified_ms: o.modified_ms ?? 0,
    mount: !!o.mount,
    protected: !!o.protected,
    readonly: !!o.readonly,
    hidden: !!o.hidden,
  };
}

export const rootsKey = (serverId: string) => ["file-roots", serverId];

export const useFileRoots = (serverId: string) =>
  useQuery({
    queryKey: rootsKey(serverId),
    queryFn: async () => {
      const r = await get<RootsResponse>(`${base(serverId)}/roots`);
      return { ...r, roots: Array.isArray(r.roots) ? r.roots : [] };
    },
    retry: false,
    staleTime: 60_000,
  });

export const useDirectory = (serverId: string, root: string, path: string, hidden: boolean, enabled = true) =>
  useInfiniteQuery({
    queryKey: ["files", serverId, root, path, hidden],
    queryFn: async ({ pageParam }) => {
      const p = await get<DirPage>(
        `${base(serverId)}/list?root=${enc(root)}&path=${enc(path)}&offset=${pageParam}&limit=${PAGE}&hidden=${hidden}`,
      );
      return { ...p, total: p.total ?? 0, entries: Array.isArray(p.entries) ? p.entries.map(normEntry) : [] };
    },
    initialPageParam: 0,
    getNextPageParam: (p) => (p.offset + p.entries.length < p.total && p.entries.length > 0 ? p.offset + p.entries.length : undefined),
    retry: false,
    staleTime: 10_000,
    enabled,
  });

export const readFile = (serverId: string, root: string, path: string, offset: number, version?: string) =>
  get<ReadResult>(
    `${base(serverId)}/read?root=${enc(root)}&path=${enc(path)}&offset=${offset}&length=${READ_CHUNK}${version ? `&version=${enc(version)}` : ""}`,
  );

/** Same-origin download link (session cookie; no credentials in the URL). */
export const downloadUrl = (serverId: string, root: string, path: string) =>
  `${base(serverId)}/download?root=${enc(root)}&path=${enc(path)}`;

export const hashFile = (serverId: string, root: string, path: string) =>
  get<{ sha256: string; size: number }>(`${base(serverId)}/hash?root=${enc(root)}&path=${enc(path)}`);

export const mkdir = (serverId: string, root: string, path: string) => post(`${base(serverId)}/mkdir`, { root, path });
export const moveEntry = (serverId: string, root: string, from: string, to: string, overwrite = false) =>
  post(`${base(serverId)}/move`, { root, from, to, overwrite });
export const copyEntry = (serverId: string, root: string, from: string, to: string, overwrite = false) =>
  post(`${base(serverId)}/copy`, { root, from, to, overwrite });
export const deleteEntry = (serverId: string, root: string, path: string, permanent = false) =>
  post<{ trash_id?: string }>(`${base(serverId)}/delete`, { root, path, permanent });

export interface SaveResult {
  path: string;
  size: number;
  version: string;
}

/** Saves editor content; `version` is the one opened (absent for a new file). 409 on conflict. */
export const saveFile = (serverId: string, root: string, path: string, content: Uint8Array, version?: string) =>
  post<SaveResult>(`${base(serverId)}/save`, { root, path, content_b64: encodeB64(content), version });

export const listTrash = (serverId: string, root: string) =>
  get<{ entries: TrashEntry[] }>(`${base(serverId)}/trash?root=${enc(root)}`).then((r) => r.entries ?? []);
export const restoreTrash = (serverId: string, root: string, id: string, overwrite = false) =>
  post(`${base(serverId)}/trash/restore`, { root, id, overwrite });
export const purgeTrash = (serverId: string, root: string, id?: string) => post(`${base(serverId)}/trash/purge`, { root, id });

export const listRevisions = (serverId: string, root: string, path: string) =>
  get<{ revisions: Revision[] }>(`${base(serverId)}/revisions?root=${enc(root)}&path=${enc(path)}`).then((r) => r.revisions ?? []);
export const readRevision = (serverId: string, root: string, path: string, revision: string) =>
  get<ReadResult>(`${base(serverId)}/revision?root=${enc(root)}&path=${enc(path)}&revision=${enc(revision)}`);

export const searchFiles = (serverId: string, root: string, path: string, spec: SearchSpec) =>
  post<SearchResult>(`${base(serverId)}/search`, { root, path, spec });

export interface NewRoot {
  kind: "service" | "container";
  label: string;
  path: string;
  container?: string;
  read_only: boolean;
}
export const addRoot = (serverId: string, r: NewRoot) => post<FileRoot>(`${base(serverId)}/roots`, r);
export const deleteRoot = (serverId: string, id: string) => api("DELETE", `${base(serverId)}/roots/${enc(id)}`);

// ---- uploads -----------------------------------------------------------------

interface UploadSession {
  id: string;
  chunk_size: number;
  size: number;
  /** Chunks the server accepts at once (it keeps them in order); absent on older servers. */
  parallel?: number;
}

export type UploadOverwrite = "never" | "always" | { version: string };

/**
 * Uploads a file in chunks with an incremental SHA-256 the node verifies.
 * Up to `parallel` chunks are in flight (the server writes them in order),
 * so sending overlaps with the node's writing (issue #15). A failed chunk is
 * retried after asking the server how far it got.
 */
export async function uploadFile(
  serverId: string,
  root: string,
  path: string,
  file: Blob,
  overwrite: UploadOverwrite,
  onProgress: (sent: number, total: number) => void,
  signal: AbortSignal,
): Promise<void> {
  const s = await post<UploadSession>(`${base(serverId)}/uploads`, { root, path, size: file.size, overwrite });
  const url = `${base(serverId)}/uploads/${enc(s.id)}`;
  const hash = new Sha256();
  const parallel = Math.max(1, Math.min(4, Math.floor(s.parallel ?? 1)));
  const send = async (offset: number, end: number, bytes: Uint8Array<ArrayBuffer>) => {
    for (let attempt = 0; attempt < 4; attempt++) {
      const res = await fetch(`${url}?offset=${offset}`, {
        method: "PUT",
        credentials: "same-origin",
        headers: { "content-type": "application/octet-stream", "x-csrf-token": csrfToken() ?? "" },
        body: bytes,
        signal,
      }).catch((e: unknown) => {
        if (signal.aborted) throw e;
        return null;
      });
      if (res?.ok) return;
      if (res && res.status < 500 && res.status !== 409) {
        const d = await res.json().catch(() => ({}));
        throw new ApiError(res.status, d.title ?? "error", d.detail ?? res.statusText);
      }
      // Network error or node hiccup: ask where the upload stands (later
      // chunks may already be there too).
      const st = await get<{ received: number }>(url).catch(() => null);
      if (st && st.received >= end) return;
    }
    throw new Error("the upload could not continue; try again");
  };
  let next = 0;
  let done = 0;
  const inFlight = new Set<Promise<void>>();
  try {
    while (next < file.size) {
      if (signal.aborted) throw new DOMException("Upload cancelled", "AbortError");
      const offset = next;
      const end = Math.min(offset + s.chunk_size, file.size);
      const bytes = new Uint8Array(await file.slice(offset, end).arrayBuffer());
      // The checksum follows the file's order, whatever order chunks finish in.
      hash.update(bytes);
      next = end;
      const p: Promise<void> = send(offset, end, bytes).then(() => {
        inFlight.delete(p);
        done += end - offset;
        onProgress(done, file.size);
      });
      // Failures surface through `race`/`all` below; never unhandled.
      p.catch(() => undefined);
      inFlight.add(p);
      if (inFlight.size >= parallel) await Promise.race(inFlight);
    }
    await Promise.all(inFlight);
    await post(`${url}/commit`, { sha256: hash.hex() });
  } catch (e) {
    void api("DELETE", url).catch(() => undefined);
    throw e;
  }
}

// ---- helpers -------------------------------------------------------------------

/** Joins a public path and a child name. */
export const joinPath = (dir: string, name: string) => (dir === "/" ? `/${name}` : `${dir}/${name}`);
export const parentPath = (p: string) => {
  const parts = p.split("/").filter(Boolean);
  parts.pop();
  return "/" + parts.join("/");
};
export const baseName = (p: string) => p.split("/").filter(Boolean).pop() ?? "/";

export function decodeB64(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

export function encodeB64(bytes: Uint8Array): string {
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
}

export type Encoding = "utf-8" | "utf-16le" | "utf-16be" | "latin1";
export type TextKind = { kind: "text"; encoding: Encoding; bom: boolean } | { kind: "binary" };

/** BOM and UTF-16 first, then a NUL-byte binary check, then UTF-8 validity. */
export function detectText(head: Uint8Array): TextKind {
  if (head[0] === 0xef && head[1] === 0xbb && head[2] === 0xbf) return { kind: "text", encoding: "utf-8", bom: true };
  if (head[0] === 0xff && head[1] === 0xfe) return { kind: "text", encoding: "utf-16le", bom: true };
  if (head[0] === 0xfe && head[1] === 0xff) return { kind: "text", encoding: "utf-16be", bom: true };
  const sample = head.subarray(0, 8192);
  if (sample.includes(0)) return { kind: "binary" };
  try {
    // A multi-byte character cut at the end of the sample is not an error.
    new TextDecoder("utf-8", { fatal: true }).decode(sample.subarray(0, Math.max(0, sample.length - 4)));
    return { kind: "text", encoding: "utf-8", bom: false };
  } catch {
    return { kind: "text", encoding: "latin1", bom: false };
  }
}

export function decodeText(bytes: Uint8Array, kind: Extract<TextKind, { kind: "text" }>): string {
  const body = kind.bom ? bytes.subarray(kind.encoding === "utf-8" ? 3 : 2) : bytes;
  return new TextDecoder(kind.encoding === "latin1" ? "windows-1252" : kind.encoding).decode(body);
}

/** Encodes text back in the file's encoding (BOM kept). Latin-1 is kept for code points < 256. */
export function encodeText(text: string, kind: Extract<TextKind, { kind: "text" }>): Uint8Array {
  if (kind.encoding === "utf-8") {
    const body = new TextEncoder().encode(text);
    if (!kind.bom) return body;
    const out = new Uint8Array(body.length + 3);
    out.set([0xef, 0xbb, 0xbf]);
    out.set(body, 3);
    return out;
  }
  if (kind.encoding === "latin1") {
    const out = new Uint8Array(text.length);
    for (let i = 0; i < text.length; i++) {
      const c = text.charCodeAt(i);
      if (c > 255) throw new Error("the text has characters Latin-1 cannot store; save it as UTF-8");
      out[i] = c;
    }
    return out;
  }
  const le = kind.encoding === "utf-16le";
  const out = new Uint8Array(2 + text.length * 2);
  out.set(le ? [0xff, 0xfe] : [0xfe, 0xff]);
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    out[2 + 2 * i] = le ? c & 0xff : c >> 8;
    out[3 + 2 * i] = le ? c >> 8 : c & 0xff;
  }
  return out;
}

/** The file's line ending, kept on save. */
export const lineEnding = (text: string): "\r\n" | "\n" => (/\r\n/.test(text.slice(0, 65536)) ? "\r\n" : "\n");

// ---- temporary SFTP access ------------------------------------------------------

export interface SftpInfo {
  enabled: boolean;
  protocol?: "SFTP";
  host?: string;
  port?: number;
  fingerprint?: string;
  max_hours?: number;
  /** `[sftp] allow_never_expiry`: access without expiry may be issued at all. */
  allow_never_expiry?: boolean;
  /** The caller may issue it here (enabled, and `file.admin`). */
  can_never_expire?: boolean;
}

export interface FileGrant {
  id: string;
  name: string;
  roots: string[];
  write: boolean;
  delete: boolean;
  created_by: string;
  /** Whose current rights the access acts with: the creator, or whoever last widened it. */
  owner: string;
  created_at: number;
  expires_at: number;
  revoked_at?: number | null;
  allowed_ips: string[];
  max_sessions: number;
  bandwidth_kbps?: number | null;
  transfer_limit?: number | null;
  bytes_transferred: number;
  last_used_at?: number | null;
  active: boolean;
  status: "active" | "expired" | "revoked";
  never_expires: boolean;
  server_id: string;
  /** Set by the fleet-wide list. */
  server_name?: string;
}

/** Changes to a grant; absent fields stay. `null` clears a limit. */
export interface GrantChanges {
  name?: string;
  roots?: string[];
  write?: boolean;
  delete?: boolean;
  expires_in_minutes?: number;
  never_expires?: boolean;
  allowed_ips?: string[];
  max_sessions?: number;
  bandwidth_kbps?: number | null;
  transfer_limit?: number | null;
}

export interface NewGrant {
  name: string;
  roots: string[];
  write: boolean;
  delete: boolean;
  expires_in_minutes: number;
  never_expires?: boolean;
  allowed_ips?: string[];
  max_sessions?: number;
  bandwidth_kbps?: number;
  transfer_limit?: number;
}

export interface IssuedGrant {
  grant: FileGrant;
  connection: { protocol: "SFTP"; host: string; port: number; username: string; password: string; expires_at: number; fingerprint: string };
}

export const sftpInfo = (serverId: string) => get<SftpInfo>(`${base(serverId)}/sftp`);
export const listGrants = (serverId: string) => get<{ grants: FileGrant[] }>(`${base(serverId)}/grants`).then((r) => r.grants ?? []);
export const createGrant = (serverId: string, g: NewGrant) => post<IssuedGrant>(`${base(serverId)}/grants`, g);
export const revokeGrant = (serverId: string, id: string) => post<FileGrant>(`${base(serverId)}/grants/${enc(id)}/revoke`, {});
export const updateGrant = (serverId: string, id: string, c: GrantChanges) => api<FileGrant>("PATCH", `${base(serverId)}/grants/${enc(id)}`, c);
export const resetGrantPassword = (serverId: string, id: string) => post<IssuedGrant>(`${base(serverId)}/grants/${enc(id)}/reset-password`, {});
/** Grants across the fleet that the caller may see. */
export const listAllGrants = (f: { server?: string; status?: string; q?: string } = {}) => {
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(f)) if (v) q.set(k, v);
  const s = q.toString();
  return get<{ grants: FileGrant[] }>(`/api/v1/files/grants${s ? `?${s}` : ""}`).then((r) => (Array.isArray(r.grants) ? r.grants : []));
};

// ---- permissions, archives and bulk actions -------------------------------------

export interface PermResult {
  path: string;
  dry_run: boolean;
  changed: number;
  unchanged: number;
  skipped: number;
  sample: { path: string; before: string; after: string }[];
}

export const chmod = (serverId: string, root: string, path: string, mode: string, recursive: boolean, dryRun: boolean) =>
  post<PermResult>(`${base(serverId)}/chmod`, { root, path, mode, recursive, dry_run: dryRun });
export const chown = (serverId: string, root: string, path: string, owner: string, group: string, recursive: boolean, dryRun: boolean) =>
  post<PermResult>(`${base(serverId)}/chown`, { root, path, owner: owner || undefined, group: group || undefined, recursive, dry_run: dryRun });
export const getAcl = (serverId: string, root: string, path: string) =>
  get<{ path: string; acl: string; mode?: string }>(`${base(serverId)}/acl?root=${enc(root)}&path=${enc(path)}`);

export type ArchiveFormat = "zip" | "tar_gz";

export interface ArchiveResult {
  dest?: string;
  temp_id?: string;
  size: number;
  entries: number;
  bytes: number;
  skipped: { path?: string; entry?: string; reason: string }[];
}

export const compress = (serverId: string, root: string, sources: string[], dest: string) =>
  post<ArchiveResult>(`${base(serverId)}/compress`, { root, sources, dest });
export const extract = (serverId: string, root: string, archive: string, dest: string) =>
  post<ArchiveResult>(`${base(serverId)}/extract`, { root, archive, dest });

export type BulkAction = "copy" | "move" | "trash" | "delete";

export interface BulkResult {
  ok: number;
  failed: number;
  results: { path: string; ok: boolean; error?: string }[];
}

export const bulk = (serverId: string, root: string, action: BulkAction, paths: string[], to?: string, overwrite = false) =>
  post<BulkResult>(`${base(serverId)}/bulk`, { root, action, paths, to, overwrite });

/** Packs a selection for download; returns the temporary archive's URL. */
export async function archiveUrl(serverId: string, root: string, paths: string[], format: ArchiveFormat = "zip") {
  const r = await post<ArchiveResult>(`${base(serverId)}/archive`, { root, paths, format });
  return `${base(serverId)}/archive/${enc(r.temp_id ?? "")}?root=${enc(root)}`;
}

/** Whether a file name looks like an archive Forge can extract. */
export const isArchive = (name: string) => /\.(zip|tar|tar\.gz|tgz)$/i.test(name);
