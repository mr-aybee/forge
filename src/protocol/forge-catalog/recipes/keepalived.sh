SERVICE=keepalived
PROC=/usr/sbin/keepalived
OS_PKG=keepalived
pkg_running() { keepalived --version 2>&1 | sed -n 's/^Keepalived v\([^ ]*\).*/\1/p'; }
pkg_install() {
  need_supported
  [ -n "$FORGE_P_VIP" ] || die "a virtual IP is required"
  os_install "$(pinned keepalived)"
  # VRRP is IP protocol 112; open it where firewalld runs.
  if command -v firewall-cmd >/dev/null 2>&1 && firewall-cmd --state >/dev/null 2>&1; then
    firewall-cmd -q --permanent --add-rich-rule='rule protocol value="vrrp" accept' && firewall-cmd -q --reload
  fi
  prio=100
  [ "$FORGE_P_STATE" = BACKUP ] && prio=90
  pass=$(pkg_secret VRRP_PASS | cut -c1-8)
  {
    echo "# Managed by Orbyn Forge"
    echo "vrrp_script chk_nginx {"
    echo "  script \"/usr/bin/pgrep -x nginx\""
    echo "  interval 2"
    echo "  fall 2"
    echo "  rise 2"
    echo "}"
    echo "vrrp_instance FORGE_EDGE {"
    echo "  state $FORGE_P_STATE"
    echo "  interface $FORGE_P_INTERFACE"
    echo "  virtual_router_id 51"
    echo "  priority $prio"
    echo "  advert_int 1"
    echo "  authentication {"
    echo "    auth_type PASS"
    echo "    auth_pass $pass"
    echo "  }"
    echo "  virtual_ipaddress {"
    echo "    $FORGE_P_VIP"
    echo "  }"
    echo "  track_script {"
    echo "    chk_nginx"
    echo "  }"
    echo "}"
  } > /etc/keepalived/keepalived.conf
  chmod 0600 /etc/keepalived/keepalived.conf
  svc enable keepalived
  svc restart keepalived
  pkg_env VIP "$FORGE_P_VIP"
}
pkg_uninstall() { svc disable keepalived; os_remove keepalived; }
