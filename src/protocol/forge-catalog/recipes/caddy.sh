# Caddy from its official repositories (Cloudsmith deb, COPR rpm). Serves
# /etc/caddy/Caddyfile; Forge only sets the default site's port.
OS_PKG=caddy
SERVICE=caddy
PROC=/usr/bin/caddy
pkg_running() { caddy version 2>/dev/null | awk '{print $1}' | sed 's/^v//'; }
pkg_after_upgrade() { svc reload caddy 2>/dev/null || svc restart caddy; }
pkg_install() {
  need_supported
  port_free "$FORGE_P_PORT"
  case "$(os_family)" in
    deb) apt_repo caddy https://dl.cloudsmith.io/public/caddy/stable/gpg.key "https://dl.cloudsmith.io/public/caddy/stable/deb/debian any-version main" ;;
    rpm) rpm_repo caddy "https://download.copr.fedorainfracloud.org/results/@caddy/caddy/epel-$EL_MAJOR-$RPM_ARCH/" \
           https://download.copr.fedorainfracloud.org/results/@caddy/caddy/pubkey.gpg ;;
  esac
  os_install "$(pinned caddy)"
  # Only the packaged welcome site is rewritten; a Caddyfile you edited stays.
  if grep -q '^:80 {' /etc/caddy/Caddyfile 2>/dev/null; then
    sed -i "s/^:80 {/:$FORGE_P_PORT {/" /etc/caddy/Caddyfile
  fi
  selinux_port http_port_t "$FORGE_P_PORT"
  fw_for_bind "$FORGE_P_PORT"
  svc enable caddy
  svc restart caddy
  pkg_env PORT "$FORGE_P_PORT"
  pkg_env CONFIG /etc/caddy/Caddyfile
}
pkg_uninstall() { svc disable caddy; fw_close "$FORGE_P_PORT"; os_remove caddy; }
