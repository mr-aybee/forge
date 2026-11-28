# Apache HTTP Server (distribution package). Listens on its own port so it
# can sit behind (or next to) Forge's Nginx edge.
case "$(os_family)" in
  rpm)
    OS_PKG=httpd; SERVICE=httpd; BIN=httpd
    UPGRADE_WITH="httpd-core httpd-tools httpd-filesystem"
    ;;
  *)
    OS_PKG=apache2; SERVICE=apache2; BIN=apache2
    UPGRADE_WITH="apache2-bin apache2-data apache2-utils"
    ;;
esac
PROC=$BIN
pkg_running() { "$BIN" -v 2>/dev/null | sed -n 's|^Server version: Apache/\([0-9.]*\).*|\1|p'; }
pkg_after_upgrade() { "$BIN" -t >/dev/null 2>&1 && svc reload "$SERVICE" 2>/dev/null || svc restart "$SERVICE"; }
pkg_install() {
  need_supported
  port_free "$FORGE_P_PORT"
  os_install "$(pinned "$OS_PKG")"
  listen="$(bind_addr):$FORGE_P_PORT"; [ "$FORGE_P_BIND" = all ] && listen="$FORGE_P_PORT"
  case "$(os_family)" in
    rpm)
      sed -i "s/^Listen .*/Listen $listen/" /etc/httpd/conf/httpd.conf
      selinux_port http_port_t "$FORGE_P_PORT"
      ;;
    *)
      sed -i "s/^Listen 80$/Listen $listen/; s/^Listen [0-9.]*:*[0-9]*$/Listen $listen/" /etc/apache2/ports.conf
      sed -i "s/<VirtualHost \*:[0-9]*>/<VirtualHost *:$FORGE_P_PORT>/" /etc/apache2/sites-available/000-default.conf
      ;;
  esac
  "$BIN" -t >/dev/null 2>&1 || die "apache configuration test failed"
  fw_for_bind "$FORGE_P_PORT"
  svc enable "$SERVICE"
  svc restart "$SERVICE"
  pkg_env PORT "$FORGE_P_PORT"
}
pkg_uninstall() {
  svc disable "$SERVICE"
  fw_close "$FORGE_P_PORT"
  case "$(os_family)" in rpm) os_remove httpd ;; *) os_remove apache2 apache2-bin apache2-data apache2-utils ;; esac
}
