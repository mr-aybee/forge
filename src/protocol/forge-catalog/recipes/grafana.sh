# Grafana OSS from Grafana Labs' repositories, admin password kept on the node.
OS_PKG=grafana
SERVICE=grafana-server
PROC=grafana
pkg_running() {
  { grafana --version 2>/dev/null || grafana-server -v 2>/dev/null; } | sed -n 's/.*[Vv]ersion \([0-9][0-9.]*\).*/\1/p' | head -1
}
pkg_install() {
  need_supported
  port_free "$FORGE_P_PORT"
  repo_grafana
  os_install "$(pinned grafana)"
  ini=/etc/grafana/grafana.ini
  sed -i "s/^;\{0,1\}http_port = .*/http_port = $FORGE_P_PORT/; s/^;\{0,1\}http_addr =.*/http_addr = $(bind_addr)/" "$ini"
  selinux_port http_port_t "$FORGE_P_PORT"
  fw_for_bind "$FORGE_P_PORT"
  svc enable grafana-server
  svc restart grafana-server
  wait_http "http://127.0.0.1:$FORGE_P_PORT/api/health" 90
  pw=$(pkg_secret PASSWORD)
  if command -v grafana >/dev/null 2>&1; then
    grafana cli --homepath /usr/share/grafana --config "$ini" admin reset-admin-password "$pw" >/dev/null 2>&1
  else
    grafana-cli --homepath /usr/share/grafana --config "$ini" admin reset-admin-password "$pw" >/dev/null 2>&1
  fi || die "could not set the Grafana admin password"
  chown -R grafana:grafana /var/lib/grafana 2>/dev/null || true
  pkg_env URL "http://127.0.0.1:$FORGE_P_PORT"
  pkg_env USER admin
}
pkg_uninstall() { svc disable grafana-server; fw_close "$FORGE_P_PORT"; os_remove grafana; }
