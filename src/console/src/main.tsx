import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { useCapabilities, useMe } from "./api/hooks";
import { PrefsProvider } from "./app/prefs";
import { router } from "./app/router";
import { ToastProvider } from "./app/toast";
import { Login } from "./pages/Login";
import "./styles.css";
import { installInputModality } from "./app/inputModality";
import { installStaleBuildHandler } from "./app/staleBuild";

installStaleBuildHandler();
installInputModality();

const queryClient = new QueryClient({
  defaultOptions: {
    // Live updates arrive over the WebSocket; polling is only a fallback.
    queries: { staleTime: 10_000, refetchInterval: 30_000, refetchOnWindowFocus: true },
  },
});

/** Shows the login page until a session exists, then the routed console. */
function Gate() {
  const me = useMe();
  const caps = useCapabilities();
  if (me.isLoading) return <div className="grid h-screen place-items-center text-muted">Loading…</div>;
  if (!me.data) {
    const sso = caps.data?.capabilities.some((c) => c.id === "auth.oidc" && c.enabled) ?? false;
    return <Login ssoEnabled={sso} />;
  }
  return <RouterProvider router={router} />;
}

const root = document.getElementById("root");
if (root) {
  createRoot(root).render(
    <StrictMode>
      <QueryClientProvider client={queryClient}>
        <PrefsProvider>
          <ToastProvider>
            <Gate />
          </ToastProvider>
        </PrefsProvider>
      </QueryClientProvider>
    </StrictMode>,
  );
}
