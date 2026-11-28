case "$(os_family)" in
  rpm) OS_PKG=redis; SERVICE=redis; CONF=/etc/redis/redis.conf ;;
  *) OS_PKG=redis-server; UPGRADE_WITH=redis-tools; SERVICE=redis-server; CONF=/etc/redis/redis.conf ;;
esac
PROC=/usr/bin/redis-server
pkg_running() { redis-server --version | sed -n 's/.* v=\([^ ]*\).*/\1/p'; }
pkg_install() {
  need_supported
  os_install "$(pinned "$OS_PKG")"
  # Older RHEL packages keep the file at /etc/redis.conf.
  [ -f "$CONF" ] || CONF=/etc/redis.conf
  pw=$(pkg_secret PASSWORD)
  sed -i "s/^port .*/port $FORGE_P_PORT/; s/^bind .*/bind $(bind_addr)/; s/^# *requirepass .*/requirepass $pw/; s/^requirepass .*/requirepass $pw/" "$CONF"
  grep -q "^requirepass " "$CONF" || echo "requirepass $pw" >> "$CONF"
  if [ "$FORGE_P_BIND" = all ]; then sed -i 's/^protected-mode .*/protected-mode no/' "$CONF"; fi
  [ "$FORGE_P_PORT" = 6379 ] || selinux_port redis_port_t "$FORGE_P_PORT"
  fw_for_bind "$FORGE_P_PORT"
  svc enable "$SERVICE"
  svc restart "$SERVICE"
  pkg_env HOST 127.0.0.1
  pkg_env PORT "$FORGE_P_PORT"
  pkg_env URL "redis://:$pw@127.0.0.1:$FORGE_P_PORT/0"
}
pkg_uninstall() {
  svc disable "$SERVICE"
  fw_close "$FORGE_P_PORT"
  case "$(os_family)" in rpm) os_remove redis ;; *) os_remove redis-server redis-tools ;; esac
}
