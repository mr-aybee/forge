# Valkey (the BSD-licensed Redis fork) from the distribution: Debian 13+,
# Ubuntu 24.10+, RHEL/Rocky/Alma with EPEL.
case "$(os_family)" in
  rpm) OS_PKG=valkey; SERVICE=valkey ;;
  *) OS_PKG=valkey-server; UPGRADE_WITH=valkey-tools; SERVICE=valkey-server ;;
esac
PROC=valkey-server
CONF=/etc/valkey/valkey.conf
pkg_running() { valkey-server --version | sed -n 's/.* v=\([^ ]*\).*/\1/p'; }
pkg_install() {
  need_supported
  port_free "$FORGE_P_PORT"
  case "$(os_family)" in
    rpm) epel_enable ;;
    deb) apt_update ;;
  esac
  [ -n "$(os_candidate "$OS_PKG")" ] || die "Valkey is not packaged for ${PRETTY_NAME:-this distribution} (Debian 13+, Ubuntu 24.10+ and RHEL-likes with EPEL have it)"
  os_install "$(pinned "$OS_PKG")"
  pw=$(pkg_secret PASSWORD)
  sed -i "s/^port .*/port $FORGE_P_PORT/; s/^bind .*/bind $(bind_addr)/; s/^# *requirepass .*/requirepass $pw/; s/^requirepass .*/requirepass $pw/" "$CONF"
  grep -q "^requirepass " "$CONF" || echo "requirepass $pw" >> "$CONF"
  if [ "$FORGE_P_BIND" = all ]; then sed -i 's/^protected-mode .*/protected-mode no/' "$CONF"; fi
  selinux_port redis_port_t "$FORGE_P_PORT"
  fw_for_bind "$FORGE_P_PORT"
  svc enable "$SERVICE"
  svc restart "$SERVICE"
  pkg_env HOST 127.0.0.1
  pkg_env PORT "$FORGE_P_PORT"
  pkg_env URL "redis://:$pw@127.0.0.1:$FORGE_P_PORT/0"
}
pkg_uninstall() { svc disable "$SERVICE"; fw_close "$FORGE_P_PORT"; os_remove valkey valkey-server valkey-tools; }
