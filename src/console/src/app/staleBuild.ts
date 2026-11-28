/**
 * After the console is rebuilt, an open tab still references the previous
 * build's chunk files, which no longer exist ("Failed to fetch dynamically
 * imported module"). Reload once to pick up the new build; the guard stops a
 * reload loop if the chunk is genuinely unreachable.
 */
const KEY = "forge.stale-reload";

export const isChunkError = (e: unknown) =>
  /Failed to fetch dynamically imported module|Importing a module script failed|error loading dynamically imported module|Unable to preload CSS/i.test(
    e instanceof Error ? e.message : String(e),
  );

/** Reloads unless one already happened in the last 30 s. Returns whether it reloads. */
export function reloadForNewBuild(): boolean {
  try {
    const last = Number(sessionStorage.getItem(KEY) ?? 0);
    if (Date.now() - last < 30_000) return false;
    sessionStorage.setItem(KEY, String(Date.now()));
  } catch {
    /* storage blocked: still reload once */
  }
  window.location.reload();
  return true;
}

export function installStaleBuildHandler() {
  window.addEventListener("vite:preloadError", (event) => {
    if (reloadForNewBuild()) event.preventDefault();
  });
}
