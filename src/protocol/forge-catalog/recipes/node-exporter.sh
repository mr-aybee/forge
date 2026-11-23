# Prometheus node exporter (host metrics) from the distribution; EPEL on RHEL.
case "$(os_family)" in
  rpm) OS_PKG=golang-github-prometheus-node-exporter; SERVICE=node_exporter; DEFAULTS=/etc/default/node_exporter ;;
  *) OS_PKG=prometheus-node-exporter; SERVICE=prometheus-node-exporter; DEFAULTS=/etc/default/prometheus-node-exporter ;;
esac
PROC=node_exporter
pkg_running() {
  { prometheus-node-exporter --version 2>&1 || node_exporter --version 2>&1; } \
    | sed -n 's/^node_exporter, version \([0-9.]*\).*/\1/p' | head -1
}
pkg_install() {
  need_supported
  port_free "$FORGE_P_PORT"
  if [ "$(os_family)" = rpm ]; then epel_enable; fi
  if [ "$(os_family)" = deb ]; then apt_update; fi
  [ -n "$(os_candidate "$OS_PKG")" ] || die "the node exporter is not packaged for ${PRETTY_NAME:-this distribution}"
  os_install "$(pinned "$OS_PKG")"
  install -d -m 0755 "$(dirname "$DEFAULTS")"
  touch "$DEFAULTS"
  grep -v '^ARGS=\|^OPTIONS=' "$DEFAULTS" > "$DEFAULTS.tmp" || true
  echo "ARGS=\"--web.listen-address=$(bind_addr):$FORGE_P_PORT\"" >> "$DEFAULTS.tmp"
  echo "OPTIONS=\"--web.listen-address=$(bind_addr):$FORGE_P_PORT\"" >> "$DEFAULTS.tmp"
  mv -f "$DEFAULTS.tmp" "$DEFAULTS"
  fw_for_bind "$FORGE_P_PORT"
  svc enable "$SERVICE"
  svc restart "$SERVICE"
  wait_http "http://127.0.0.1:$FORGE_P_PORT/metrics" 30
  pkg_env URL "http://127.0.0.1:$FORGE_P_PORT/metrics"
}
pkg_uninstall() { svc disable "$SERVICE"; fw_close "$FORGE_P_PORT"; os_remove "$OS_PKG"; }
