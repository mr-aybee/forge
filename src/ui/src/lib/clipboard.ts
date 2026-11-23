/** Shown when neither the Clipboard API nor the fallback can copy. */
export const COPY_BLOCKED = "Copy blocked by the browser — select and press Ctrl+C";

/**
 * Copies text to the clipboard. Uses the Clipboard API on secure origins and
 * falls back to a hidden textarea + `execCommand("copy")` where it is not
 * available (consoles served over plain HTTP on a LAN address). Returns false
 * when the browser blocks both. Never logs the text: one-time secrets are
 * copied through this too.
 */
export async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    /* fall through to the legacy path */
  }
  if (typeof document === "undefined") return false;
  const active = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const ta = document.createElement("textarea");
  ta.value = text;
  ta.setAttribute("readonly", "");
  ta.setAttribute("aria-hidden", "true");
  ta.style.position = "fixed";
  ta.style.top = "0";
  ta.style.left = "0";
  ta.style.opacity = "0";
  // Radix dialogs trap focus: put the textarea inside the open dialog so it can take the selection.
  const host = active?.closest<HTMLElement>('[role="dialog"]') ?? document.body;
  host.appendChild(ta);
  ta.focus();
  ta.select();
  let ok = false;
  try {
    ok = document.execCommand("copy");
  } catch {
    ok = false;
  }
  ta.remove();
  active?.focus();
  return ok;
}
