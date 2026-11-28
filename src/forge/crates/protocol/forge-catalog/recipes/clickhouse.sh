# ClickHouse from packages.clickhouse.com; `lts` or `stable` channel.
OS_PKG=clickhouse-server
UPGRADE_WITH="clickhouse-common-static clickhouse-client"
SERVICE=clickhouse-server
PROC=clickhouse-server
pkg_running() { clickhouse-client --version 2>/dev/null | sed -n 's/.*version \([0-9.]*\).*/\1/p'; }
pkg_install() {
  need_supported
  port_free "$FORGE_P_HTTP_PORT"
  port_free "$FORGE_P_PORT"
  case "$(os_family)" in
    deb) apt_repo clickhouse https://packages.clickhouse.com/rpm/lts/repodata/repomd.xml.key "https://packages.clickhouse.com/deb $VERSION main" ;;
    rpm) rpm_repo clickhouse "https://packages.clickhouse.com/rpm/$VERSION/" "https://packages.clickhouse.com/rpm/$VERSION/repodata/repomd.xml.key" ;;
  esac
  os_install "$(pinned clickhouse-server)" "$(pinned clickhouse-client)" "$(pinned clickhouse-common-static)"
  pw=$(pkg_secret PASSWORD)
  hash=$(printf '%s' "$pw" | sha256sum | cut -d' ' -f1)
  install -d -m 0750 /etc/clickhouse-server/config.d /etc/clickhouse-server/users.d
  listen=127.0.0.1; [ "$FORGE_P_BIND" = all ] && listen=0.0.0.0
  cat > /etc/clickhouse-server/config.d/90-forge.xml <<XML
<clickhouse>
    <listen_host>$listen</listen_host>
    <http_port>$FORGE_P_HTTP_PORT</http_port>
    <tcp_port>$FORGE_P_PORT</tcp_port>
</clickhouse>
XML
  # The packaged `default` user has no password; give it Forge's.
  cat > /etc/clickhouse-server/users.d/90-forge.xml <<XML
<clickhouse>
    <users>
        <default>
            <password remove="1"/>
            <password_sha256_hex>$hash</password_sha256_hex>
        </default>
    </users>
</clickhouse>
XML
  rm -f /etc/clickhouse-server/users.d/default-password.xml
  chown -R clickhouse:clickhouse /etc/clickhouse-server/config.d /etc/clickhouse-server/users.d
  chmod 0640 /etc/clickhouse-server/users.d/90-forge.xml
  fw_for_bind "$FORGE_P_HTTP_PORT"
  fw_for_bind "$FORGE_P_PORT"
  svc enable clickhouse-server
  svc restart clickhouse-server
  wait_http "http://127.0.0.1:$FORGE_P_HTTP_PORT/ping" 90
  pkg_env HOST 127.0.0.1
  pkg_env PORT "$FORGE_P_PORT"
  pkg_env HTTP_PORT "$FORGE_P_HTTP_PORT"
  pkg_env USER default
}
pkg_uninstall() {
  svc disable clickhouse-server
  fw_close "$FORGE_P_HTTP_PORT"; fw_close "$FORGE_P_PORT"
  os_remove clickhouse-server clickhouse-client clickhouse-common-static
}
