# RabbitMQ and a matching Erlang from Team RabbitMQ's repositories, with the
# management UI, an admin user `forge_admin` and the guest user removed.
OS_PKG=rabbitmq-server
SERVICE=rabbitmq-server
PROC=beam.smp
KEYS=https://github.com/rabbitmq/signing-keys/releases/download/3.0
pkg_running() { rabbitmqctl -q version 2>/dev/null; }
pkg_install() {
  need_supported
  port_free "$FORGE_P_PORT"
  case "$(os_family)" in
    deb)
      case "$OS_ID" in *ubuntu*) dist=ubuntu ;; *) dist=debian ;; esac
      apt_repo rabbitmq-erlang "$KEYS/cloudsmith.rabbitmq-erlang.E495BB49CC4BBE5B.key" \
        "https://ppa1.rabbitmq.com/rabbitmq/rabbitmq-erlang/deb/$dist $CODENAME main"
      apt_repo rabbitmq-server "$KEYS/cloudsmith.rabbitmq-server.9F4587F226208342.key" \
        "https://ppa1.rabbitmq.com/rabbitmq/rabbitmq-server/deb/$dist $CODENAME main"
      ;;
    rpm)
      rpm_repo rabbitmq-erlang "https://yum1.rabbitmq.com/erlang/el/$EL_MAJOR/$RPM_ARCH" \
        "$KEYS/cloudsmith.rabbitmq-erlang.E495BB49CC4BBE5B.key"
      rpm_repo rabbitmq-server "https://yum1.rabbitmq.com/rabbitmq/el/$EL_MAJOR/noarch" \
        "$KEYS/cloudsmith.rabbitmq-server.9F4587F226208342.key"
      ;;
  esac
  os_install "$(pinned rabbitmq-server)"
  install -d -m 0755 /etc/rabbitmq
  {
    echo "# Managed by Orbyn Forge"
    echo "listeners.tcp.default = $(bind_addr):$FORGE_P_PORT"
    echo "management.tcp.port = $FORGE_P_UI_PORT"
    echo "management.tcp.ip = $(bind_addr)"
    echo "loopback_users.guest = true"
  } > /etc/rabbitmq/rabbitmq.conf
  rabbitmq-plugins --offline enable rabbitmq_management >/dev/null
  [ "$FORGE_P_PORT" = 5672 ] || selinux_port amqp_port_t "$FORGE_P_PORT"
  fw_for_bind "$FORGE_P_PORT"
  fw_for_bind "$FORGE_P_UI_PORT"
  svc enable rabbitmq-server
  svc restart rabbitmq-server
  i=0
  until rabbitmqctl -q status >/dev/null 2>&1; do i=$((i + 1)); [ "$i" -lt 90 ] || die "rabbitmq did not start"; sleep 1; done
  pw=$(pkg_secret PASSWORD)
  rabbitmqctl -q add_user forge_admin "$pw" >/dev/null 2>&1 || rabbitmqctl -q change_password forge_admin "$pw" >/dev/null
  rabbitmqctl -q set_user_tags forge_admin administrator >/dev/null
  rabbitmqctl -q set_permissions -p / forge_admin '.*' '.*' '.*' >/dev/null
  rabbitmqctl -q delete_user guest >/dev/null 2>&1 || true
  pkg_env HOST 127.0.0.1
  pkg_env PORT "$FORGE_P_PORT"
  pkg_env USER forge_admin
  pkg_env URL "amqp://forge_admin:$pw@127.0.0.1:$FORGE_P_PORT/"
  pkg_env UI "http://127.0.0.1:$FORGE_P_UI_PORT"
}
pkg_uninstall() {
  svc disable rabbitmq-server
  fw_close "$FORGE_P_PORT"; fw_close "$FORGE_P_UI_PORT"
  os_remove rabbitmq-server
}
