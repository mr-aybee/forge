SERVICE=docker
OS_PKG=docker-ce
UPGRADE_WITH=docker-ce-cli
pkg_running() { docker version --format '{{.Server.Version}}' 2>/dev/null; }
# Containers restart with the daemon; say so in the job log.
pkg_after_upgrade() { log "restarting the Docker daemon; containers with a restart policy come back"; svc restart docker; }
pkg_install() {
  need_supported
  case "$(os_family)" in
    deb)
      case "$OS_ID" in *ubuntu*) dist=ubuntu ;; *) dist=debian ;; esac
      apt_repo docker "https://download.docker.com/linux/$dist/gpg" "https://download.docker.com/linux/$dist $CODENAME stable"
      # The distribution's own Docker packages share files with Docker CE
      # (docker-buildx, containerd), so apt refuses Docker CE next to them.
      # Remove the installed ones first, as Docker's install guide says.
      # `remove`, not purge: images, containers and volumes in /var/lib/docker stay.
      conflicts=""
      for p in docker.io docker-doc docker-compose docker-compose-v2 docker-buildx podman-docker containerd runc; do
        dpkg-query -W -f='${Status}' "$p" 2>/dev/null | grep -q 'install ok installed' && conflicts="$conflicts $p"
      done
      if [ -n "$conflicts" ]; then
        log "removing the distribution's Docker packages that conflict with Docker CE:$conflicts"
        # shellcheck disable=SC2086 # one word per package (and per lock option)
        DEBIAN_FRONTEND=noninteractive apt-get $APT_LOCK remove -y -q $conflicts >/dev/null
      fi
      apt_install "$(pinned docker-ce)" docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
      ;;
    rpm)
      rpm_repo_file docker https://download.docker.com/linux/rhel/docker-ce.repo
      # Podman's docker shim and runc conflict with Docker CE.
      "$DNF_BIN" install -y -q --allowerasing "$(pinned docker-ce)" docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin >/dev/null
      ;;
  esac
  svc enable docker
  pkg_env BIN /usr/bin/docker
  pkg_env SOCKET /var/run/docker.sock
}
pkg_uninstall() {
  svc disable docker
  os_remove docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
}
