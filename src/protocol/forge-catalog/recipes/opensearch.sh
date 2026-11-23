# OpenSearch from artifacts.opensearch.org (major version track), single
# node, security plugin on with an `admin` password kept on the node.
OS_PKG=opensearch
SERVICE=opensearch
PROC=org.opensearch.bootstrap.OpenSearch
pkg_running() {
  curl -sk -m 5 -u "admin:$(pkg_env_get FORGE_PKG_OPENSEARCH_PASSWORD)" "https://127.0.0.1:$FORGE_P_PORT" \
    | sed -n 's/.*"number" *: *"\([^"]*\)".*/\1/p'
}
pkg_install() {
  need_supported
  need_systemd
  port_free "$FORGE_P_PORT"
  key=https://artifacts.opensearch.org/publickeys/opensearch-release.pgp
  case "$(os_family)" in
    deb) apt_repo opensearch "$key" "https://artifacts.opensearch.org/releases/bundle/opensearch/$VERSION.x/apt stable main" ;;
    rpm) rpm_repo opensearch "https://artifacts.opensearch.org/releases/bundle/opensearch/$VERSION.x/yum" "$key" ;;
  esac
  # The package's install script needs a strong initial admin password.
  pw="$(pkg_env_get FORGE_PKG_OPENSEARCH_PASSWORD)"
  [ -n "$pw" ] || pw="Fg1!$(gen_secret)"
  pkg_env PASSWORD "$pw"
  OPENSEARCH_INITIAL_ADMIN_PASSWORD="$pw" os_install "$(pinned opensearch)"
  conf=/etc/opensearch/opensearch.yml
  sed -i '/^# forge-managed/,$d' "$conf"
  {
    echo "# forge-managed (below)"
    echo "http.port: $FORGE_P_PORT"
    echo "http.host: $(bind_addr)"
    echo "discovery.type: single-node"
  } >> "$conf"
  printf -- '-Xms%s\n-Xmx%s\n' "$FORGE_P_HEAP" "$FORGE_P_HEAP" > /etc/opensearch/jvm.options.d/90-forge.options
  fw_for_bind "$FORGE_P_PORT"
  svc enable opensearch
  svc restart opensearch
  wait_http "https://127.0.0.1:$FORGE_P_PORT" 180
  pkg_env URL "https://127.0.0.1:$FORGE_P_PORT"
  pkg_env USER admin
}
pkg_uninstall() { svc disable opensearch; fw_close "$FORGE_P_PORT"; os_remove opensearch; }
