OS_PKG=python3
pkg_running() { python3 -c 'import platform; print(platform.python_version())'; }
pkg_after_upgrade() { log "running Python services pick up the new interpreter on their next restart"; }
pkg_install() {
  need_supported
  case "$(os_family)" in
    rpm) os_install python3 python3-pip python3-devel gcc make ;;
    *) apt_install python3 python3-venv python3-pip python3-dev build-essential ;;
  esac
  pkg_env BIN /usr/bin/python3
}
pkg_uninstall() { log "the system Python is shared with the OS and is left in place"; }
pkg_status() { command -v python3 >/dev/null && echo running || echo not-installed; }
