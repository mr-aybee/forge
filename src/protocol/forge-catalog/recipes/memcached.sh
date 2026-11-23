OS_PKG=memcached
SERVICE=memcached
PROC=/usr/bin/memcached
pkg_running() { memcached -V | awk '{print $2}'; }
pkg_install() {
  need_supported
  port_free "$FORGE_P_PORT"
  os_install "$(pinned memcached)"
  case "$(os_family)" in
    rpm)
      cat > /etc/sysconfig/memcached <<CONF
# Managed by Orbyn Forge
PORT="$FORGE_P_PORT"
USER="memcached"
MAXCONN="1024"
CACHESIZE="$FORGE_P_MEMORY_MB"
OPTIONS="-l $(bind_addr) -U 0"
CONF
      ;;
    *)
      cat > /etc/memcached.conf <<CONF
# Managed by Orbyn Forge
-d
logfile /var/log/memcached.log
-m $FORGE_P_MEMORY_MB
-p $FORGE_P_PORT
-u memcache
-l $(bind_addr)
-U 0
-P /var/run/memcached/memcached.pid
CONF
      ;;
  esac
  [ "$FORGE_P_PORT" = 11211 ] || selinux_port memcache_port_t "$FORGE_P_PORT"
  fw_for_bind "$FORGE_P_PORT"
  svc enable memcached
  svc restart memcached
  pkg_env HOST 127.0.0.1
  pkg_env PORT "$FORGE_P_PORT"
}
pkg_uninstall() { svc disable memcached; fw_close "$FORGE_P_PORT"; os_remove memcached; }
