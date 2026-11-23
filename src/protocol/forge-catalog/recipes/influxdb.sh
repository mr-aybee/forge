# InfluxDB 2 OSS from repos.influxdata.com, set up with an admin user, an
# organisation `forge`, a bucket `default` and an operator token.
OS_PKG=influxdb2
UPGRADE_WITH=influxdb2-cli
SERVICE=influxdb
PROC=influxd
pkg_running() { influxd version 2>/dev/null | sed -n 's/^InfluxDB v\([0-9.]*\).*/\1/p'; }
pkg_install() {
  need_supported
  port_free "$FORGE_P_PORT"
  case "$(os_family)" in
    deb) apt_repo influxdata https://repos.influxdata.com/influxdata-archive.key "https://repos.influxdata.com/debian stable main" ;;
    rpm) rpm_repo influxdata "https://repos.influxdata.com/stable/$RPM_ARCH/main" https://repos.influxdata.com/influxdata-archive.key ;;
  esac
  os_install "$(pinned influxdb2)" influxdb2-cli
  addr=127.0.0.1; [ "$FORGE_P_BIND" = all ] && addr=""
  conf=/etc/influxdb/config.toml
  touch "$conf"
  grep -v '^http-bind-address' "$conf" > "$conf.tmp" || true
  echo "http-bind-address = \"$addr:$FORGE_P_PORT\"" >> "$conf.tmp"
  mv -f "$conf.tmp" "$conf"
  fw_for_bind "$FORGE_P_PORT"
  svc enable influxdb
  svc restart influxdb
  url="http://127.0.0.1:$FORGE_P_PORT"
  wait_http "$url/health" 60
  pw=$(pkg_secret PASSWORD)
  if influx setup --host "$url" --username forge_admin --password "$pw" --org forge --bucket default --force >/dev/null 2>&1; then
    token=$(influx auth list --host "$url" --json 2>/dev/null | sed -n 's/.*"token": "\([^"]*\)".*/\1/p' | head -1)
    [ -n "$token" ] && pkg_env TOKEN "$token"
  fi
  pkg_env URL "$url"
  pkg_env ORG forge
  pkg_env BUCKET default
  pkg_env USER forge_admin
}
pkg_uninstall() { svc disable influxdb; fw_close "$FORGE_P_PORT"; os_remove influxdb2 influxdb2-cli; }
