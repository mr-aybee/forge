import { CheckCircle2, TriangleAlert, X } from "lucide-react";
import { createContext, useCallback, useContext, useState, type ReactNode } from "react";

type Tone = "ok" | "error";
/** An optional follow-up shown as a link in the toast ("Open Ingress"). */
export interface ToastAction {
  label: string;
  onSelect: () => void;
}
interface Toast {
  id: number;
  tone: Tone;
  text: string;
  action?: ToastAction;
}

const Ctx = createContext<(text: string, tone?: Tone, action?: ToastAction) => void>(() => {});

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const push = useCallback((text: string, tone: Tone = "ok", action?: ToastAction) => {
    const id = Date.now() + Math.random();
    setToasts((t) => [...t.slice(-3), { id, tone, text, action }]);
    // A toast with a link stays a little longer so it can be clicked.
    setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), tone === "error" ? 8000 : action ? 7000 : 4000);
  }, []);
  return (
    <Ctx.Provider value={push}>
      {children}
      <div aria-live="polite" className="fixed right-4 bottom-4 z-50 flex w-[min(380px,calc(100vw-32px))] flex-col gap-2">
        {toasts.map((t) => (
          <div
            key={t.id}
            className={`animate-in flex items-start gap-2 rounded-lg border bg-panel px-3 py-2.5 shadow-[var(--shadow-pop)] ${
              t.tone === "ok" ? "border-ok/40" : "border-bad/40"
            }`}
          >
            {t.tone === "ok" ? (
              <CheckCircle2 className="mt-0.5 size-4 shrink-0 text-ok" />
            ) : (
              <TriangleAlert className="mt-0.5 size-4 shrink-0 text-bad" />
            )}
            <span className="flex-1">
              {t.text}
              {t.action && (
                <>
                  {" "}
                  <button
                    type="button"
                    className="font-medium text-accent hover:underline"
                    onClick={() => {
                      t.action?.onSelect();
                      setToasts((x) => x.filter((y) => y.id !== t.id));
                    }}
                  >
                    {t.action.label}
                  </button>
                </>
              )}
            </span>
            <button aria-label="Dismiss" onClick={() => setToasts((x) => x.filter((y) => y.id !== t.id))}>
              <X className="size-4 text-muted" />
            </button>
          </div>
        ))}
      </div>
    </Ctx.Provider>
  );
}

export const useToast = () => useContext(Ctx);

/** Formats any thrown value for a toast. */
export const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));
