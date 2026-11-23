# .NET from the OS package repositories where they carry this version
# (Microsoft's repository on Debian, the Ubuntu archive, RHEL AppStream), so
# updates arrive through apt/dnf. Falls back to Microsoft's dotnet-install.sh
# into /opt/forge/pkgs/dotnet/<version> (updated by reinstalling).
case "$FORGE_P_FLAVOR" in
  sdk) DOTNET_PKG="dotnet-sdk-$VERSION" ;;
  runtime) DOTNET_PKG="dotnet-runtime-$VERSION" ;;
  *) DOTNET_PKG="aspnetcore-runtime-$VERSION" ;;
esac
# Set OS_PKG only when the package manager owns this install.
OS_PKG=""
if [ -n "$(os_installed "$DOTNET_PKG" 2>/dev/null)" ]; then OS_PKG="$DOTNET_PKG"; fi
pkg_running() { dotnet --list-runtimes 2>/dev/null | awk -v v="$VERSION." 'index($2, v) == 1 {print $2}' | tail -1; }
ms_repo() {
  case "$(os_family)" in
    deb)
      is_debian_like || return 0
      case "$OS_ID" in *ubuntu*) return 0 ;; esac  # Ubuntu ships .NET in its own archive
      [ -f /etc/apt/sources.list.d/microsoft-prod.list ] && return 0
      fetch "https://packages.microsoft.com/config/debian/${VERSION_ID%%.*}/packages-microsoft-prod.deb" /tmp/forge-ms-prod.deb
      dpkg -i /tmp/forge-ms-prod.deb >/dev/null; rm -f /tmp/forge-ms-prod.deb
      APT_UPDATED=""
      ;;
  esac
}
pkg_install() {
  need_supported
  ms_repo
  if [ "$(os_family)" = deb ]; then apt_update; fi
  if [ -n "$(os_candidate "$DOTNET_PKG")" ]; then
    os_install "$(pinned "$DOTNET_PKG")"
    OS_PKG="$DOTNET_PKG"
    # Moving from an earlier dotnet-install.sh install: drop it, or its
    # /usr/local/bin/dotnet link would shadow the packaged /usr/bin/dotnet.
    if [ "$(readlink /usr/local/bin/dotnet 2>/dev/null)" = "$PKG_HOME/dotnet" ]; then
      rm -f /usr/local/bin/dotnet
      log "replaced the dotnet-install.sh copy in $PKG_HOME with the $DOTNET_PKG package"
    fi
    rm -rf "$PKG_HOME"
    sed -i '/^FORGE_PKG_DOTNET_HOME=/d; /^FORGE_PKG_DOTNET_BIN=/d' "$PKG_ENV" 2>/dev/null || true
    home="$(dirname "$(readlink -f "$(command -v dotnet)")")"
    pkg_env HOME "$home"
    pkg_env BIN /usr/bin/dotnet
    return
  fi
  log "no $DOTNET_PKG package for this distribution; using dotnet-install.sh (updates by reinstalling)"
  case "$(os_family)" in
    deb) apt_install libicu-dev zlib1g ;;
    rpm) os_install libicu zlib ;;
  esac
  fetch https://dot.net/v1/dotnet-install.sh /tmp/forge-dotnet-install.sh
  install -d -m 0755 "$PKG_HOME"
  if [ "$FORGE_P_FLAVOR" = sdk ]; then
    bash /tmp/forge-dotnet-install.sh --channel "$VERSION" --install-dir "$PKG_HOME" --no-path >/dev/null
  else
    bash /tmp/forge-dotnet-install.sh --channel "$VERSION" --runtime "$FORGE_P_FLAVOR" --install-dir "$PKG_HOME" --no-path >/dev/null
  fi
  rm -f /tmp/forge-dotnet-install.sh
  ln -sf "$PKG_HOME/dotnet" /usr/local/bin/dotnet
  pkg_env HOME "$PKG_HOME"
  pkg_env BIN "$PKG_HOME/dotnet"
}
pkg_uninstall() {
  if [ -n "$OS_PKG" ]; then os_remove "$OS_PKG"; fi
  rm -rf "$PKG_HOME"
  if [ "$(readlink /usr/local/bin/dotnet 2>/dev/null)" = "$PKG_HOME/dotnet" ]; then rm -f /usr/local/bin/dotnet; fi
}
pkg_status() { { [ -n "$OS_PKG" ] || [ -x "$PKG_HOME/dotnet" ]; } && echo running || echo not-installed; }
