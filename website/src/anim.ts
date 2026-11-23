import { useEffect, useRef } from "react";
import { createScope, onScroll, type Scope } from "animejs";

/** True when the visitor asked the OS for less motion. */
export function prefersReducedMotion() {
  return typeof window !== "undefined" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/**
 * Runs anime.js animations scoped to one element. Selectors inside `setup`
 * resolve within that element, and everything is reverted on unmount.
 * With reduced motion the static, pre-rendered state stays as it is.
 */
export function useAnime<T extends HTMLElement | SVGSVGElement>(setup: (root: T, scope: Scope) => void) {
  const ref = useRef<T>(null);
  useEffect(() => {
    const root = ref.current;
    if (!root || prefersReducedMotion()) return;
    const scope = createScope({ root }).add((s) => setup(root, s as Scope));
    return () => scope.revert();
    // setup is a stable module-level function in every caller.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return ref;
}

/** Start an animation once its element scrolls into view. */
export const whenVisible = (target: Element) =>
  onScroll({ target, enter: "bottom-=10% top", repeat: false });
