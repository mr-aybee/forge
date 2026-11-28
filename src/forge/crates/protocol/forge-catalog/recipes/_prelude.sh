#!/bin/sh
# Orbyn Forge recipe prelude. Runs as root via the node helper with a clean
# environment; see forge-catalog for the contract.
set -eu
umask 022
export DEBIAN_FRONTEND=noninteractive
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

ID="$FORGE_PKG_ID"
VERSION="$FORGE_PKG_VERSION"
PKG_HOME="/opt/forge/pkgs/$ID/$VERSION"
PKG_DATA="/var/lib/forge/pkgs/$ID"
PKG_ENV="/etc/forge/pkgs/$ID.env"
SERVICE=""        # recipes set the service unit name
PROC=""           # process pattern for status without systemd (default: SERVICE)
CONTAINER=""      # or the container name, for container packages
OS_PKG=""         # the OS package whose version is the installed build (apt/dnf)
UPGRADE_WITH=""   # sibling packages released with OS_PKG (same build), upgraded together
BUILD="${FORGE_PKG_BUILD:-}"  # exact build to install or upgrade to (validated)

log() { printf 'forge: %s\n' "$*"; }
# step <command…>: shows the command about to run, shell style (`$ …`), in
# the console. Never pass secrets to it: they would be shown.
step() { printf '$ %s\n' "$*"; }
die() { printf 'forge: error: %s\n' "$*" >&2; exit 1; }

# ---------------------------------------------------------------- platform
if [ -r /etc/os-release ]; then . /etc/os-release; fi
OS_ID="${ID_LIKE:-} ${ID:-}"
ID="$FORGE_PKG_ID"   # os-release also defines ID
CODENAME="${VERSION_CODENAME:-}"
VERSION="$FORGE_PKG_VERSION"
case "$(uname -m)" in
  x86_64|amd64) ARCH=amd64; GOARCH=amd64 ;;
  aarch64|arm64) ARCH=arm64; GOARCH=arm64 ;;
  *) die "unsupported architecture $(uname -m)" ;;
esac

is_debian_like() { case "$OS_ID" in *debian*|*ubuntu*) return 0 ;; esac; return 1; }
is_rhel_like() { case "$OS_ID" in *rhel*|*fedora*|*centos*|*rocky*|*almalinux*|*" ol") return 0 ;; esac; return 1; }
need_debian() { is_debian_like || die "this package supports Debian and Ubuntu only (found: ${PRETTY_NAME:-unknown})"; }
# deb (Debian, Ubuntu) or rpm (RHEL, Rocky, Alma, Fedora); empty otherwise.
os_family() { if is_debian_like; then echo deb; elif is_rhel_like; then echo rpm; fi; }
need_supported() { [ -n "$(os_family)" ] || die "unsupported distribution: ${PRETTY_NAME:-unknown} (Debian, Ubuntu, RHEL, Rocky and Alma are supported)"; }
# Major release on RHEL-likes (9 for 9.4); repositories are per major.
EL_MAJOR="${VERSION_ID%%.*}"
RPM_ARCH="$(uname -m)"
has_systemd() { [ -d /run/systemd/system ]; }

# ---------------------------------------------------------------- packages
# Wait up to five minutes for another apt or dpkg run (unattended-upgrades,
# another install) instead of failing on the dpkg lock. dnf waits by itself.
APT_LOCK="-o DPkg::Lock::Timeout=300"
APT_UPDATED=""
apt_update() { [ -n "$APT_UPDATED" ] || { step apt-get update; apt-get $APT_LOCK update -q; APT_UPDATED=1; }; }
apt_install() { need_debian; apt_update; step apt-get install -y "$@"; apt-get $APT_LOCK install -y -q --no-install-recommends "$@"; }
apt_remove() { apt-get $APT_LOCK purge -y -qq "$@" >/dev/null 2>&1 || true; apt-get $APT_LOCK autoremove -y -qq >/dev/null 2>&1 || true; }
installed_deb() { dpkg-query -W -f='${Status}' "$1" 2>/dev/null | grep -q "install ok installed"; }

# apt_repo <name> <key-url> <deb-line-without-signed-by>
# The repository is checked at once (only its own index is fetched); one
# that fails (bad key, wrong suite, unreachable) is removed again, so it can
# never block later apt runs on the node.
apt_repo() {
  need_debian
  apt_install ca-certificates curl gnupg
  install -d -m 0755 /etc/apt/keyrings
  _key="/etc/apt/keyrings/forge-$1.gpg"
  _list="/etc/apt/sources.list.d/forge-$1.list"
  step curl -fsSL "$2" "| gpg --dearmor  # $1 repository key"
  curl -fsSL --retry 3 "$2" | gpg --dearmor --yes -o "$_key.new" \
    || { rm -f "$_key.new"; die "cannot download the signing key for the $1 repository ($2)"; }
  mv -f "$_key.new" "$_key"
  chmod 0644 "$_key"
  echo "deb [signed-by=$_key] $3" > "$_list"
  if ! _err="$(apt-get $APT_LOCK update -qq -o Dir::Etc::sourcelist="$_list" -o Dir::Etc::sourceparts=- \
        -o APT::Get::List-Cleanup=0 2>&1 >/dev/null)" || printf '%s' "$_err" | grep -qE '^(E|W):.*(not signed|NO_PUBKEY|EXPKEYSIG|404|Release file)'; then
    rm -f "$_list" "$_key"
    die "the $1 repository could not be used and was removed: $(printf '%s' "$_err" | grep -E '^(E|W):' | head -2 | tr '\n' ' ')"
  fi
  APT_UPDATED=""
}

# ---------------------------------------------------------------- versions
# The node's package manager: apt, dnf or yum (empty if none).
pkg_mgr() {
  if is_debian_like && command -v apt-get >/dev/null 2>&1; then echo apt
  elif command -v dnf >/dev/null 2>&1; then echo dnf
  elif command -v yum >/dev/null 2>&1; then echo yum
  fi
}
# pinned <pkg>: the package spec for the requested build (or the package).
pinned() {
  if [ -z "$BUILD" ]; then echo "$1"; return; fi
  case "$(pkg_mgr)" in apt) echo "$1=$BUILD" ;; *) echo "$1-$BUILD" ;; esac
}
# Refreshes package indexes at most once an hour (read-only otherwise).
os_refresh() {
  case "$(pkg_mgr)" in
    apt) [ -n "$(find /var/lib/apt/lists -maxdepth 0 -mmin -60 2>/dev/null)" ] || apt-get $APT_LOCK update -qq >/dev/null 2>&1 || true ;;
    dnf|yum) "$(pkg_mgr)" -q makecache >/dev/null 2>&1 || true ;;
  esac
}
os_installed() {
  case "$(pkg_mgr)" in
    apt) dpkg-query -W -f='${Status} ${Version}' "$1" 2>/dev/null | sed -n 's/^install ok installed //p' ;;
    dnf|yum) rpm -q --qf '%|EPOCH?{%{EPOCH}:}:{}|%{VERSION}-%{RELEASE}\n' "$1" 2>/dev/null | grep -v 'not installed' | tail -1 || true ;;
  esac
}
os_candidate() {
  case "$(pkg_mgr)" in
    apt) apt-cache policy "$1" 2>/dev/null | awk '/Candidate:/ {print $2}' | grep -v '(none)' || true ;;
    dnf|yum) "$(pkg_mgr)" -q repoquery --latest-limit 1 --arch "$RPM_ARCH,noarch" --qf '%{evr}' "$1" 2>/dev/null | tail -1 || true ;;
  esac
}
# Upgrades OS_PKG (and installed UPGRADE_WITH siblings) to BUILD, or to the
# newest candidate, without touching other packages.
os_upgrade() {
  _pkgs="$(pinned "$1")"
  _cur="$(os_installed "$1")"
  for _p in $UPGRADE_WITH; do
    # Only siblings on the same build as the main package move with it.
    if [ -n "$_cur" ] && [ "$(os_installed "$_p")" = "$_cur" ]; then _pkgs="$_pkgs $(pinned "$_p")"; fi
  done
  case "$(pkg_mgr)" in
    # shellcheck disable=SC2086
    apt) apt_update; apt-get $APT_LOCK install -y -qq --only-upgrade --no-install-recommends            -o Dpkg::Options::=--force-confold $_pkgs >/dev/null ;;
    # shellcheck disable=SC2086
    dnf|yum) "$(pkg_mgr)" -y -q upgrade $_pkgs >/dev/null ;;  # changed configs are kept (.rpmnew)
    *) die "no supported package manager on this node" ;;
  esac
}
json_str() { printf '"%s"' "$(printf '%s' "$1" | tr -cd 'A-Za-z0-9.+~:_ ()/-' | cut -c1-120)"; }
# Recipes may override: prints the version the running service reports.
pkg_running() { :; }
# Recipes may override: what to do after the OS package was upgraded.
pkg_after_upgrade() { if [ -n "$CONTAINER$SERVICE" ]; then pkg_restart; fi; }
pkg_versions() {
  if [ -z "$OS_PKG" ]; then echo '{"manager":"manual"}'; return; fi
  os_refresh
  _run="$(pkg_running 2>/dev/null | head -1 || true)"
  printf '{"manager":%s,"package":%s,"installed":%s,"candidate":%s,"running":%s}
'     "$(json_str "$(pkg_mgr)")" "$(json_str "$OS_PKG")" "$(json_str "$(os_installed "$OS_PKG")")"     "$(json_str "$(os_candidate "$OS_PKG")")" "$(json_str "$_run")"
}

# ---------------------------------------------------------------- deb + rpm
DNF_BIN="$(command -v dnf || command -v yum || true)"
# os_install <packages…>: apt or dnf, whichever the node has.
os_install() {
  case "$(os_family)" in
    deb) apt_install "$@" ;;
    rpm) step "$(basename "$DNF_BIN")" install -y "$@"; "$DNF_BIN" install -y "$@" ;;
    *) need_supported ;;
  esac
}
os_remove() {
  case "$(os_family)" in
    deb) apt_remove "$@" ;;
    rpm) "$DNF_BIN" remove -y -q "$@" >/dev/null 2>&1 || true ;;
  esac
}
# rpm_repo <name> <baseurl> <gpg-key-url>
rpm_repo() {
  cat > "/etc/yum.repos.d/forge-$1.repo" <<REPO
[forge-$1]
name=forge-$1
baseurl=$2
enabled=1
gpgcheck=1
gpgkey=$3
REPO
  # Checked at once; a repository dnf cannot use is removed again.
  if ! "$DNF_BIN" -q makecache --disablerepo='*' --enablerepo="forge-$1" >/dev/null 2>&1; then
    rm -f "/etc/yum.repos.d/forge-$1.repo"
    die "the $1 repository ($2) could not be used and was removed"
  fi
}
# rpm_repo_file <name> <url of a .repo file>
rpm_repo_file() { curl -fsSL --retry 3 "$2" -o "/etc/yum.repos.d/forge-$1.repo"; }
# EPEL (and CRB, which EPEL packages often need) on RHEL-likes.
epel_enable() {
  rpm -q epel-release >/dev/null 2>&1 && return 0
  "$DNF_BIN" install -y -q epel-release >/dev/null 2>&1 \
    || "$DNF_BIN" install -y -q "https://dl.fedoraproject.org/pub/epel/epel-release-latest-$EL_MAJOR.noarch.rpm" >/dev/null
  "$DNF_BIN" config-manager --set-enabled crb >/dev/null 2>&1 \
    || { "$DNF_BIN" install -y -q dnf-plugins-core >/dev/null 2>&1 && "$DNF_BIN" config-manager --set-enabled crb >/dev/null 2>&1; } || true
}
# Opens a TCP port in an active host firewall (firewalld or ufw); no-op otherwise.
fw_open() {
  if command -v firewall-cmd >/dev/null 2>&1 && firewall-cmd --state >/dev/null 2>&1; then
    firewall-cmd -q --permanent --add-port="$1/tcp" && firewall-cmd -q --reload
  elif command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -q '^Status: active'; then
    ufw allow "$1/tcp" >/dev/null
  fi
}
fw_close() {
  if command -v firewall-cmd >/dev/null 2>&1 && firewall-cmd --state >/dev/null 2>&1; then
    firewall-cmd -q --permanent --remove-port="$1/tcp" 2>/dev/null && firewall-cmd -q --reload
  elif command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -q '^Status: active'; then
    ufw delete allow "$1/tcp" >/dev/null 2>&1 || true
  fi
}
# Exposed services: open the port only when bound to all interfaces.
fw_for_bind() { if [ "${FORGE_P_BIND:-localhost}" = all ]; then fw_open "$1"; else fw_close "$1"; fi; }
selinux_on() { command -v getenforce >/dev/null 2>&1 && [ "$(getenforce)" != Disabled ]; }
# Lets a service listen on a non-default port under SELinux (type e.g. postgresql_port_t).
selinux_port() {
  selinux_on || return 0
  command -v semanage >/dev/null 2>&1 || os_install policycoreutils-python-utils
  semanage port -a -t "$1" -p tcp "$2" 2>/dev/null || semanage port -m -t "$1" -p tcp "$2" 2>/dev/null || true
}

# port_free <port>: on first install, refuse a port another program listens on.
port_free() {
  [ -n "$OS_PKG" ] && [ -n "$(os_installed "$OS_PKG")" ] && return 0  # reinstall/repair
  if command -v ss >/dev/null 2>&1; then
    _who="$(ss -ltnpH "sport = :$1" 2>/dev/null | sed -n 's/.*users:(("\([^"]*\)".*/\1/p' | head -1)"
    [ -z "$(ss -ltnH "sport = :$1" 2>/dev/null)" ] || die "port $1 is already in use${_who:+ by $_who}; choose another port"
  fi
}
need_systemd() { has_systemd || die "$ID needs systemd on the node"; }
# wait_http <url> [seconds]: until the URL answers (any HTTP status).
wait_http() {
  _i=0
  until curl -sk -o /dev/null -m 3 "$1"; do
    _i=$((_i + 1)); [ "$_i" -lt "${2:-60}" ] || die "$ID did not answer on $1"; sleep 1
  done
}
# Grafana Labs repository (Grafana, Loki).
repo_grafana() {
  case "$(os_family)" in
    deb) apt_repo grafana https://apt.grafana.com/gpg.key "https://apt.grafana.com stable main" ;;
    rpm) rpm_repo grafana https://rpm.grafana.com https://rpm.grafana.com/gpg.key ;;
  esac
}

# fetch <url> <dest>
fetch() { command -v curl >/dev/null 2>&1 || os_install ca-certificates curl; curl -fsSL --retry 3 "$1" -o "$2"; }

# ---------------------------------------------------------------- services
svc() { # svc start|stop|restart|enable|disable|status <name>
  _a="$1"; _n="$2"
  if has_systemd; then
    case "$_a" in
      status) systemctl is-active --quiet "$_n" && echo running || echo stopped ;;
      enable) step systemctl enable --now "$_n"; systemctl enable --now "$_n" >/dev/null 2>&1 ;;
      disable) step systemctl disable --now "$_n"; systemctl disable --now "$_n" >/dev/null 2>&1 || true ;;
      *) step systemctl "$_a" "$_n"; systemctl "$_a" "$_n" ;;
    esac
  else
    # No systemd (containers): SysV scripts shipped by the packages.
    case "$_a" in
      status)
        if service "$_n" status >/dev/null 2>&1 || pgrep -f "${PROC:-$_n}" >/dev/null 2>&1; then echo running; else echo stopped; fi ;;
      enable) service "$_n" start >/dev/null 2>&1 || service "$_n" restart ;;
      disable) service "$_n" stop >/dev/null 2>&1 || true ;;
      *) service "$_n" "$_a" ;;
    esac
  fi
}

# ---------------------------------------------------------------- containers
need_docker() { command -v docker >/dev/null 2>&1 || die "Docker is not installed on this node; install the Docker Engine package first"; }
ctr_status() { docker inspect -f '{{.State.Running}}' "$1" 2>/dev/null | grep -q true && echo running || echo stopped; }

# ---------------------------------------------------------------- env file
gen_secret() { head -c 24 /dev/urandom | base64 | tr -d '/+=\n' | cut -c1-28; }
pkg_env_get() { if [ -f "$PKG_ENV" ]; then sed -n "s/^$1=//p" "$PKG_ENV" | tail -1; fi; }
# pkg_env KEY VALUE — sets FORGE_PKG_<ID>_<KEY> in the package env file.
pkg_env() {
  install -d -m 0755 /etc/forge/pkgs
  _k="FORGE_PKG_$(echo "$ID" | tr 'a-z-' 'A-Z_')_$1"
  touch "$PKG_ENV"; chmod 0600 "$PKG_ENV"
  grep -v "^$_k=" "$PKG_ENV" > "$PKG_ENV.tmp" || true
  printf '%s=%s\n' "$_k" "$2" >> "$PKG_ENV.tmp"
  mv -f "$PKG_ENV.tmp" "$PKG_ENV"; chmod 0600 "$PKG_ENV"
}
# Reuses a generated secret across re-installs.
pkg_secret() {
  _k="FORGE_PKG_$(echo "$ID" | tr 'a-z-' 'A-Z_')_$1"
  _v="$(pkg_env_get "$_k")"
  [ -n "$_v" ] || _v="$(gen_secret)"
  pkg_env "$1" "$_v"
  printf '%s' "$_v"
}
bind_addr() { [ "${FORGE_P_BIND:-localhost}" = all ] && echo 0.0.0.0 || echo 127.0.0.1; }

# ---------------------------------------------------------------- defaults
# Runtimes (no service, no container) have nothing to start or stop.
pkg_start() { if [ -n "$CONTAINER" ]; then step docker start "$CONTAINER"; docker start "$CONTAINER" >/dev/null; elif [ -n "$SERVICE" ]; then svc start "$SERVICE"; fi; }
pkg_stop() { if [ -n "$CONTAINER" ]; then step docker stop "$CONTAINER"; docker stop "$CONTAINER" >/dev/null; elif [ -n "$SERVICE" ]; then svc stop "$SERVICE"; fi; }
pkg_restart() { if [ -n "$CONTAINER" ]; then step docker restart "$CONTAINER"; docker restart "$CONTAINER" >/dev/null; elif [ -n "$SERVICE" ]; then svc restart "$SERVICE"; fi; }
pkg_status() {
  if [ -n "$CONTAINER" ]; then
    command -v docker >/dev/null 2>&1 && docker inspect "$CONTAINER" >/dev/null 2>&1 || { echo not-installed; return; }
    ctr_status "$CONTAINER"
  elif [ -n "$SERVICE" ]; then
    svc status "$SERVICE"
  else
    [ -d "$PKG_HOME" ] || [ -f "$PKG_ENV" ] && echo running || echo not-installed
  fi
}

forge_dispatch() {
  case "${1:-}" in
    install)
      install -d -m 0755 "$PKG_DATA" /etc/forge/pkgs
      pkg_install
      pkg_env VERSION "$VERSION"
      log "installed $ID $VERSION"
      pkg_status ;;
    uninstall) pkg_uninstall; rm -f "$PKG_ENV"; log "uninstalled $ID"; echo not-installed ;;
    start) pkg_start; pkg_status ;;
    stop) pkg_stop; pkg_status ;;
    restart) pkg_restart; pkg_status ;;
    status) pkg_status ;;
    versions) pkg_versions ;;
    upgrade)
      [ -n "$OS_PKG" ] || die "$ID is not installed from a package repository; update it manually"
      [ -n "$(os_installed "$OS_PKG")" ] || die "$OS_PKG is not installed"
      os_upgrade "$OS_PKG"
      pkg_after_upgrade
      _now="$(os_installed "$OS_PKG")"
      [ -z "$BUILD" ] || [ "$_now" = "$BUILD" ] || die "$OS_PKG is at $_now after the upgrade, not $BUILD"
      log "upgraded $OS_PKG to $_now"
      log "state: $(pkg_status)"
      pkg_versions ;;
    *) die "unknown action ${1:-}" ;;
  esac
}
