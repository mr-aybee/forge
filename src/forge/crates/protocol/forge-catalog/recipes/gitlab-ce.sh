# GitLab Community Edition (Omnibus) from packages.gitlab.com. Brings its own
# Nginx, PostgreSQL and Redis under /opt/gitlab; needs 4 GB RAM or more.
OS_PKG=gitlab-ce
SERVICE=gitlab-runsvdir
PROC=/opt/gitlab/embedded/bin/runsvdir
pkg_running() { cat /opt/gitlab/embedded/service/gitlab-rails/VERSION 2>/dev/null; }
# The package runs `gitlab-ctl reconfigure` and restarts what changed itself.
pkg_after_upgrade() { gitlab-ctl status >/dev/null 2>&1 || gitlab-ctl restart >/dev/null; }
pkg_start() { gitlab-ctl start >/dev/null; }
pkg_stop() { gitlab-ctl stop >/dev/null; }
pkg_restart() { gitlab-ctl restart >/dev/null; }
pkg_status() {
  command -v gitlab-ctl >/dev/null 2>&1 || { echo not-installed; return; }
  gitlab-ctl status puma >/dev/null 2>&1 && echo running || echo stopped
}
pkg_install() {
  need_supported
  need_systemd
  port_free "$FORGE_P_PORT"
  mem_mb=$(awk '/MemTotal/ {print int($2 / 1024)}' /proc/meminfo)
  [ "$mem_mb" -ge 3800 ] || die "GitLab needs at least 4 GB of RAM (this node has ${mem_mb} MB)"
  case "$(os_family)" in
    deb)
      case "$OS_ID" in *ubuntu*) dist=ubuntu ;; *) dist=debian ;; esac
      apt_repo gitlab https://packages.gitlab.com/gitlab/gitlab-ce/gpgkey "https://packages.gitlab.com/gitlab/gitlab-ce/$dist/ $CODENAME main"
      ;;
    rpm)
      rpm_repo gitlab "https://packages.gitlab.com/gitlab/gitlab-ce/el/$EL_MAJOR/$RPM_ARCH" https://packages.gitlab.com/gitlab/gitlab-ce/gpgkey
      ;;
  esac
  host="${FORGE_P_HOST:-$(hostname -f 2>/dev/null || hostname)}"
  url="http://$host:$FORGE_P_PORT"
  pw="$(pkg_env_get FORGE_PKG_GITLAB_CE_ROOT_PASSWORD)"
  [ -n "$pw" ] || pw="Fg1!$(gen_secret)"
  pkg_env ROOT_PASSWORD "$pw"
  EXTERNAL_URL="$url" GITLAB_ROOT_PASSWORD="$pw" os_install "$(pinned gitlab-ce)"
  if ! grep -q "^external_url '$url'" /etc/gitlab/gitlab.rb; then
    sed -i "s|^external_url .*|external_url '$url'|" /etc/gitlab/gitlab.rb
    gitlab-ctl reconfigure >/dev/null
  fi
  fw_open "$FORGE_P_PORT"
  wait_http "http://127.0.0.1:$FORGE_P_PORT/users/sign_in" 600
  pkg_env URL "$url"
  pkg_env USER root
}
pkg_uninstall() {
  gitlab-ctl stop >/dev/null 2>&1 || true
  fw_close "$FORGE_P_PORT"
  os_remove gitlab-ce
}
