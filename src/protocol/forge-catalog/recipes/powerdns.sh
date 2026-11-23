SERVICE=pdns
PROC=pdns_server
case "$(os_family)" in
  rpm) OS_PKG=pdns; UPGRADE_WITH=pdns-backend-sqlite; PDNS_CONF_DIR=/etc/pdns ;;
  *) OS_PKG=pdns-server; UPGRADE_WITH=pdns-backend-sqlite3; PDNS_CONF_DIR=/etc/powerdns ;;
esac
pkg_running() { pdns_control version 2>/dev/null; }
pkg_install() {
  need_supported
  case "$(os_family)" in
    deb)
      apt_install "$(pinned pdns-server)" pdns-backend-sqlite3 sqlite3
      schema=/usr/share/pdns-backend-sqlite3/schema/schema.sqlite3.sql
      conf="$PDNS_CONF_DIR/pdns.d/90-forge.conf"
      rm -f "$PDNS_CONF_DIR/pdns.d/bind.conf"
      ;;
    rpm)
      epel_enable
      os_install "$(pinned pdns)" pdns-backend-sqlite sqlite
      schema="$(rpm -ql pdns-backend-sqlite pdns 2>/dev/null | grep '/schema.sqlite3.sql$' | head -1)"
      # Minimal images skip documentation files (tsflags=nodocs); the schema
      # is one of them.
      if [ -z "$schema" ] || [ ! -f "$schema" ]; then
        "$DNF_BIN" reinstall -y -q --setopt=tsflags= pdns-backend-sqlite pdns >/dev/null 2>&1 || true
        schema="$(rpm -ql pdns-backend-sqlite pdns 2>/dev/null | grep '/schema.sqlite3.sql$' | head -1)"
      fi
      # RHEL's pdns.conf has no include directory by default.
      install -d -m 0755 "$PDNS_CONF_DIR/pdns.d"
      grep -q '^include-dir=' "$PDNS_CONF_DIR/pdns.conf" || echo "include-dir=$PDNS_CONF_DIR/pdns.d" >> "$PDNS_CONF_DIR/pdns.conf"
      sed -i 's/^launch=.*/# &  (set by Forge in pdns.d)/' "$PDNS_CONF_DIR/pdns.conf"
      conf="$PDNS_CONF_DIR/pdns.d/90-forge.conf"
      ;;
  esac
  [ -n "$schema" ] && [ -f "$schema" ] || die "PowerDNS sqlite schema not found"
  db="$PKG_DATA/pdns.sqlite3"
  [ -f "$db" ] || sqlite3 "$db" < "$schema"
  chown -R pdns:pdns "$PKG_DATA"
  if selinux_on; then
    chcon -R -t named_cache_t "$PKG_DATA" 2>/dev/null || true
  fi
  key=$(pkg_secret API_KEY)
  {
    echo "launch=gsqlite3"
    echo "gsqlite3-database=$db"
    echo "api=yes"
    echo "api-key=$key"
    echo "webserver=yes"
    echo "webserver-address=127.0.0.1"
    echo "webserver-port=$FORGE_P_API_PORT"
    echo "webserver-allow-from=127.0.0.1,::1"
  } > "$conf"
  chmod 0640 "$conf"
  chgrp pdns "$conf"
  fw_open 53
  if command -v firewall-cmd >/dev/null 2>&1 && firewall-cmd --state >/dev/null 2>&1; then
    firewall-cmd -q --permanent --add-port=53/udp && firewall-cmd -q --reload
  fi
  pdns_up
  pkg_env API_URL "http://127.0.0.1:$FORGE_P_API_PORT"
}
# pdns-server ships only a systemd unit; without systemd run it directly.
pdns_up() {
  if has_systemd; then svc enable pdns; svc restart pdns; return; fi
  pdns_down
  pdns_server --daemon=yes --guardian=yes --config-dir="$PDNS_CONF_DIR"
  i=0
  until pdns_control rping >/dev/null 2>&1; do i=$((i + 1)); [ "$i" -lt 20 ] || die "pdns_server did not start"; sleep 1; done
}
pdns_down() {
  if has_systemd; then svc stop pdns; return; fi
  pdns_control quit >/dev/null 2>&1 || true
  i=0
  while pgrep -x pdns_server >/dev/null 2>&1 && [ "$i" -lt 10 ]; do i=$((i + 1)); sleep 1; done
}
pkg_start() { pdns_up; }
pkg_restart() { pdns_up; }
pkg_stop() { pdns_down; }
pkg_after_upgrade() { pdns_up; }
pkg_uninstall() {
  pdns_down
  svc disable pdns
  case "$(os_family)" in
    rpm) os_remove pdns pdns-backend-sqlite ;;
    *) os_remove pdns-server pdns-backend-sqlite3 ;;
  esac
  rm -f "$PDNS_CONF_DIR/pdns.d/90-forge.conf"
}
