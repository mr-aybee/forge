import type { Cluster } from "../api/types";

const hue = (s: string) => [...s].reduce((h, c) => (h * 31 + c.charCodeAt(0)) % 360, 7);

type BadgeInput = Pick<Cluster, "name"> & { abbr?: string; icon?: string | null };

/** Initials for a cluster: its own abbreviation, else from the name ("lab" → LA, "edge-eu" → EE). */
export function initials(c: BadgeInput) {
  if (c.abbr?.trim()) return c.abbr.trim().toUpperCase().slice(0, 4);
  const parts = c.name.split(/[\s\-_.]+/).filter(Boolean);
  return ((parts.length > 1 ? parts[0]![0]! + parts[1]![0]! : c.name.slice(0, 2)) || "?").toUpperCase();
}

/** Square cluster badge: the uploaded image, or initials on a colour derived from the name. */
export function ClusterBadge({ c, size = 36, className = "" }: { c: BadgeInput; size?: number; className?: string }) {
  const text = initials(c);
  const h = hue(c.name || "?");
  return c.icon ? (
    <img src={c.icon} alt="" className={`shrink-0 rounded-[10px] object-cover ${className}`} style={{ width: size, height: size }} />
  ) : (
    <span
      aria-hidden
      className={`grid shrink-0 place-items-center rounded-[10px] font-bold tracking-wide text-white ${className}`}
      style={{
        width: size,
        height: size,
        fontSize: size * (text.length > 2 ? 0.28 : 0.36),
        background: `linear-gradient(140deg, hsl(${h} 55% 48%), hsl(${(h + 35) % 360} 60% 32%))`,
      }}
    >
      {text}
    </span>
  );
}

/** Read an image file as a data URL; raster images are scaled down to at most 128 px. */
export async function imageToDataUrl(file: File): Promise<string> {
  const raw = await new Promise<string>((ok, fail) => {
    const r = new FileReader();
    r.onload = () => ok(String(r.result));
    r.onerror = () => fail(r.error);
    r.readAsDataURL(file);
  });
  if (file.type === "image/svg+xml") return raw;
  const img = new Image();
  img.src = raw;
  await img.decode();
  const scale = Math.min(1, 128 / Math.max(img.width, img.height));
  const canvas = document.createElement("canvas");
  canvas.width = Math.round(img.width * scale);
  canvas.height = Math.round(img.height * scale);
  canvas.getContext("2d")?.drawImage(img, 0, 0, canvas.width, canvas.height);
  return canvas.toDataURL("image/png");
}
