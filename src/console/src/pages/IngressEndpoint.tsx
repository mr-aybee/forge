// The ingress endpoint editor as a page (`/ingress/endpoints/$siteId`).
import { useNavigate, useParams, useRouter } from "@tanstack/react-router";

import { EndpointEditor } from "../components/ingress/EndpointEditor";

export function IngressEndpoint() {
  const { siteId } = useParams({ from: "/ingress/endpoints/$siteId" });
  const router = useRouter();
  const navigate = useNavigate();
  // Back to wherever the editor was opened from (Ingress or a cluster), else Ingress.
  const close = () => (window.history.length > 1 ? router.history.back() : void navigate({ to: "/ingress" }));
  return <EndpointEditor siteId={siteId} onClose={close} />;
}
