# Eclipse Mosquitto MQTT broker (distribution package; EPEL on RHEL), with
# anonymous access off and a user `forge` whose password is kept on the node.
OS_PKG=mosquitto
SERVICE=mosquitto
PROC=/usr/sbin/mosquitto
pkg_running() { mosquitto -h 2>/dev/null | sed -n 's/^mosquitto version \([0-9.]*\).*/\1/p'; }
pkg_install() {
  need_supported
  port_free "$FORGE_P_PORT"
  if [ "$(os_family)" = rpm ]; then epel_enable; fi
  case "$(os_family)" in
    deb) os_install "$(pinned mosquitto)" mosquitto-clients ;;
    rpm) os_install "$(pinned mosquitto)" ;;
  esac
  pw=$(pkg_secret PASSWORD)
  passwd=/etc/mosquitto/forge.passwd
  touch "$passwd"
  mosquitto_passwd -b "$passwd" forge "$pw"
  chown mosquitto:mosquitto "$passwd" 2>/dev/null || true
  chmod 0600 "$passwd"
  install -d -m 0755 /etc/mosquitto/conf.d
  # RHEL's mosquitto.conf does not read conf.d.
  grep -q '^include_dir /etc/mosquitto/conf.d' /etc/mosquitto/mosquitto.conf \
    || echo 'include_dir /etc/mosquitto/conf.d' >> /etc/mosquitto/mosquitto.conf
  {
    echo "# Managed by Orbyn Forge"
    echo "listener $FORGE_P_PORT $(bind_addr)"
    echo "allow_anonymous false"
    echo "password_file $passwd"
  } > /etc/mosquitto/conf.d/90-forge.conf
  fw_for_bind "$FORGE_P_PORT"
  svc enable mosquitto
  svc restart mosquitto
  pkg_env HOST 127.0.0.1
  pkg_env PORT "$FORGE_P_PORT"
  pkg_env USER forge
}
pkg_uninstall() { svc disable mosquitto; fw_close "$FORGE_P_PORT"; os_remove mosquitto mosquitto-clients; }
