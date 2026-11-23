# Grafana Loki (log store) from Grafana Labs' repositories, single binary
# with the packaged local-filesystem config.
OS_PKG=loki
SERVICE=loki
PROC=/usr/bin/loki
pkg_running() { loki --version 2>/dev/null | sed -n 's/^loki, version \([0-9.]*\).*/\1/p'; }
pkg_install() {
  need_supported
  port_free "$FORGE_P_PORT"
  repo_grafana
  os_install "$(pinned loki)"
  conf=/etc/loki/config.yml
  sed -i "s/^\( *\)http_listen_port:.*/\1http_listen_port: $FORGE_P_PORT/" "$conf"
  if grep -q 'http_listen_address:' "$conf"; then
    sed -i "s/^\( *\)http_listen_address:.*/\1http_listen_address: $(bind_addr)/" "$conf"
  else
    sed -i "s/^\( *\)http_listen_port:.*/&\n\1http_listen_address: $(bind_addr)/" "$conf"
  fi
  fw_for_bind "$FORGE_P_PORT"
  svc enable loki
  svc restart loki
  wait_http "http://127.0.0.1:$FORGE_P_PORT/ready" 90
  pkg_env URL "http://127.0.0.1:$FORGE_P_PORT"
}
pkg_uninstall() { svc disable loki; fw_close "$FORGE_P_PORT"; os_remove loki; }
