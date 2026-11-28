# Prometheus server from the distribution (Debian/Ubuntu; EPEL on RHEL where
# packaged). Scrapes itself and any node exporters you add to its config.
case "$(os_family)" in
  rpm) OS_PKG=golang-github-prometheus; SERVICE=prometheus; CONF=/etc/prometheus/prometheus.yml; DEFAULTS=/etc/sysconfig/prometheus ;;
  *) OS_PKG=prometheus; SERVICE=prometheus; CONF=/etc/prometheus/prometheus.yml; DEFAULTS=/etc/default/prometheus ;;
esac
PROC=/usr/bin/prometheus
pkg_running() { prometheus --version 2>&1 | sed -n 's/^prometheus, version \([0-9.]*\).*/\1/p'; }
pkg_install() {
  need_supported
  port_free "$FORGE_P_PORT"
  if [ "$(os_family)" = rpm ]; then epel_enable; fi
  if [ "$(os_family)" = deb ]; then apt_update; fi
  [ -n "$(os_candidate "$OS_PKG")" ] || die "Prometheus is not packaged for ${PRETTY_NAME:-this distribution}"
  os_install "$(pinned "$OS_PKG")"
  # Listen address and port go in the service's argument file.
  touch "$DEFAULTS"
  grep -v '^ARGS=' "$DEFAULTS" > "$DEFAULTS.tmp" || true
  echo "ARGS=\"--web.listen-address=$(bind_addr):$FORGE_P_PORT\"" >> "$DEFAULTS.tmp"
  mv -f "$DEFAULTS.tmp" "$DEFAULTS"
  # Keep the self-scrape target on the chosen port.
  [ -f "$CONF" ] && sed -i "s/localhost:9090/localhost:$FORGE_P_PORT/" "$CONF"
  fw_for_bind "$FORGE_P_PORT"
  svc enable "$SERVICE"
  svc restart "$SERVICE"
  wait_http "http://127.0.0.1:$FORGE_P_PORT/-/ready" 60
  pkg_env URL "http://127.0.0.1:$FORGE_P_PORT"
  pkg_env CONFIG "$CONF"
}
pkg_uninstall() { svc disable "$SERVICE"; fw_close "$FORGE_P_PORT"; os_remove "$OS_PKG"; }
