# Elasticsearch from artifacts.elastic.co (major version track), single node,
# security on (TLS + `elastic` user with a password kept on the node).
OS_PKG=elasticsearch
SERVICE=elasticsearch
PROC=org.elasticsearch.bootstrap.Elasticsearch
ES=/usr/share/elasticsearch/bin
pkg_running() {
  curl -sk -m 5 -u "elastic:$(pkg_env_get FORGE_PKG_ELASTICSEARCH_PASSWORD)" "https://127.0.0.1:$FORGE_P_PORT" \
    | sed -n 's/.*"number" *: *"\([^"]*\)".*/\1/p'
}
pkg_install() {
  need_supported
  need_systemd
  port_free "$FORGE_P_PORT"
  case "$(os_family)" in
    deb) apt_repo elastic https://artifacts.elastic.co/GPG-KEY-elasticsearch "https://artifacts.elastic.co/packages/$VERSION.x/apt stable main" ;;
    rpm) rpm_repo elastic "https://artifacts.elastic.co/packages/$VERSION.x/yum" https://artifacts.elastic.co/GPG-KEY-elasticsearch ;;
  esac
  os_install "$(pinned elasticsearch)"
  printf -- '-Xms%s\n-Xmx%s\n' "$FORGE_P_HEAP" "$FORGE_P_HEAP" > /etc/elasticsearch/jvm.options.d/90-forge.options
  conf=/etc/elasticsearch/elasticsearch.yml
  sed -i '/^# forge-managed/,$d' "$conf"
  # HTTP binds where asked; transport stays on loopback (single node, no
  # production bootstrap checks).
  {
    echo "# forge-managed (below)"
    echo "http.port: $FORGE_P_PORT"
    echo "http.host: $(bind_addr)"
  } >> "$conf"
  fw_for_bind "$FORGE_P_PORT"
  svc enable elasticsearch
  svc restart elasticsearch
  wait_http "https://127.0.0.1:$FORGE_P_PORT" 180
  if [ -z "$(pkg_env_get FORGE_PKG_ELASTICSEARCH_PASSWORD)" ]; then
    pw=$("$ES/elasticsearch-reset-password" -u elastic -b -s --url "https://127.0.0.1:$FORGE_P_PORT" 2>/dev/null | tail -1)
    [ -n "$pw" ] || die "could not set the elastic password"
    pkg_env PASSWORD "$pw"
  fi
  pkg_env URL "https://127.0.0.1:$FORGE_P_PORT"
  pkg_env USER elastic
  pkg_env CA_CERT /etc/elasticsearch/certs/http_ca.crt
}
pkg_uninstall() { svc disable elasticsearch; fw_close "$FORGE_P_PORT"; os_remove elasticsearch; }
