/**
 * Records whether the last interaction was a pointer press or the keyboard on
 * `<html data-input>`, so styles.css can hide focus outlines on clicked
 * buttons and tabs while keeping them for keyboard users. Installed once at
 * start-up; the listeners live as long as the page.
 */
export function installInputModality() {
  const root = document.documentElement;
  window.addEventListener("pointerdown", () => (root.dataset.input = "pointer"), { capture: true, passive: true });
  window.addEventListener(
    "keydown",
    (e) => {
      // Modifier-only presses (e.g. Ctrl+click) are not keyboard navigation.
      if (!["Shift", "Control", "Alt", "Meta"].includes(e.key)) delete root.dataset.input;
    },
    { capture: true },
  );
}
