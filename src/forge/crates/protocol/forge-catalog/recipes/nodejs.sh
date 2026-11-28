OS_PKG=nodejs
pkg_running() { node --version | sed 's/^v//'; }
pkg_install() {
  need_supported
  case "$(os_family)" in
    deb)
      apt_repo nodesource https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key "https://deb.nodesource.com/node_$VERSION.x nodistro main"
      ;;
    rpm)
      rpm_repo nodesource "https://rpm.nodesource.com/pub_$VERSION.x/nodistro/nodejs/$RPM_ARCH" \
        https://rpm.nodesource.com/gpgkey/ns-operations-public.key
      # The distribution's Node.js module would shadow NodeSource.
      "$DNF_BIN" -qy module disable nodejs >/dev/null 2>&1 || true
      ;;
  esac
  os_install "$(pinned nodejs)"
  node --version | grep -q "^v$VERSION\." || die "installed Node.js $(node --version), not $VERSION.x"
  pkg_env BIN /usr/bin/node
}
pkg_uninstall() {
  os_remove nodejs
  rm -f /etc/apt/sources.list.d/forge-nodesource.list /etc/yum.repos.d/forge-nodesource.repo
}
pkg_status() { command -v node >/dev/null && echo running || echo not-installed; }
