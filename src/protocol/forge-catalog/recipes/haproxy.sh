# HAProxy (distribution package). Forge installs and updates it; the
# configuration (/etc/haproxy/haproxy.cfg) stays yours.
OS_PKG=haproxy
SERVICE=haproxy
PROC=/usr/sbin/haproxy
pkg_running() { haproxy -v 2>/dev/null | sed -n 's/^HAProxy version \([0-9.]*\).*/\1/p'; }
pkg_after_upgrade() { haproxy -c -f /etc/haproxy/haproxy.cfg >/dev/null 2>&1 && svc reload haproxy 2>/dev/null || svc restart haproxy; }
pkg_install() {
  need_supported
  os_install "$(pinned haproxy)"
  # SELinux: let HAProxy connect to backends on any port.
  if selinux_on; then setsebool -P haproxy_connect_any 1 >/dev/null 2>&1 || true; fi
  haproxy -c -f /etc/haproxy/haproxy.cfg >/dev/null 2>&1 || die "haproxy configuration check failed"
  svc enable haproxy
  svc restart haproxy
  pkg_env CONFIG /etc/haproxy/haproxy.cfg
}
pkg_uninstall() { svc disable haproxy; os_remove haproxy; }
