# Debian/Ubuntu: Sury packages php<v>-*; RHEL-likes: Remi's parallel-installable
# php<vv>-php-* collections (several versions side by side, like Sury).
V2="$(echo "$VERSION" | tr -d .)"
case "$(os_family)" in
  rpm)
    OS_PKG="php$V2-php-fpm"
    SERVICE="php$V2-php-fpm"
    PROC="php$V2-php-fpm"
    PHPBIN="/opt/remi/php$V2/root/usr/bin/php"
    FPM_SOCKET="/var/opt/remi/php$V2/run/php-fpm/www.sock"
    ;;
  *)
    OS_PKG="php$VERSION-fpm"
    SERVICE="php$VERSION-fpm"
    PROC="/etc/php/$VERSION/fpm/php-fpm.conf"
    PHPBIN="/usr/bin/php$VERSION"
    FPM_SOCKET="/run/php/php$VERSION-fpm.sock"
    ;;
esac
# Every php<v> package of one version comes from the same source build.
UPGRADE_WITH="$(case "$(pkg_mgr)" in
  apt) dpkg-query -W -f='${Package}\n' "php$VERSION-*" 2>/dev/null ;;
  dnf|yum) rpm -qa --qf '%{NAME}\n' "php$V2-php-*" "php$V2-runtime" 2>/dev/null ;;
esac | grep -vx "$OS_PKG" | tr '\n' ' ')"
pkg_running() { "$PHPBIN" -r 'echo PHP_VERSION;'; }
# Extension package names on RHEL (Remi) where they differ from Debian's.
rpm_ext() {
  case "$1" in
    mysql) echo mysqlnd ;;
    curl|opcache) ;;  # built into php-common / installed separately
    zip) echo pecl-zip ;;
    redis) echo pecl-redis6 ;;
    *) echo "$1" ;;
  esac
}
pkg_install() {
  need_supported
  case "$(os_family)" in
    deb)
      apt_update
      if [ -z "$(os_candidate "php$VERSION-fpm")" ]; then
        case "$OS_ID" in
          *ubuntu*) apt_install software-properties-common; add-apt-repository -y ppa:ondrej/php >/dev/null; APT_UPDATED="" ;;
          *) apt_repo sury-php https://packages.sury.org/php/apt.gpg "https://packages.sury.org/php/ $CODENAME main" ;;
        esac
      fi
      exts=""
      for e in $(echo "$FORGE_P_EXTENSIONS" | tr ',' ' '); do
        [ "$e" = opcache ] && continue
        exts="$exts php$VERSION-$e"
      done
      # shellcheck disable=SC2086
      apt_install "$(pinned "php$VERSION-fpm")" "php$VERSION-cli" "php$VERSION-opcache" $exts
      ;;
    rpm)
      if ! rpm -q remi-release >/dev/null 2>&1; then
        epel_enable
        "$DNF_BIN" install -y -q "https://rpms.remirepo.net/enterprise/remi-release-$EL_MAJOR.rpm" >/dev/null
      fi
      exts=""
      for e in $(echo "$FORGE_P_EXTENSIONS" | tr ',' ' '); do
        r="$(rpm_ext "$e")"
        if [ -n "$r" ]; then exts="$exts php$V2-php-$r"; fi
      done
      # shellcheck disable=SC2086
      os_install "$(pinned "php$V2-php-fpm")" "php$V2-php-cli" "php$V2-php-opcache" $exts
      # Socket for Nginx (or Apache when Nginx is not installed yet).
      sock_user=apache; id nginx >/dev/null 2>&1 && sock_user=nginx
      sed -i "s|^listen = .*|listen = $FPM_SOCKET|; s|^;\?listen.owner = .*|listen.owner = $sock_user|; s|^;\?listen.group = .*|listen.group = $sock_user|; s|^;\?listen.mode = .*|listen.mode = 0660|; s|^listen.acl_users|;listen.acl_users|" \
        "/etc/opt/remi/php$V2/php-fpm.d/www.conf"
      ln -sf "$PHPBIN" "/usr/bin/php$VERSION"
      ;;
  esac
  svc enable "$SERVICE"
  svc restart "$SERVICE"
  pkg_env "FPM_SOCKET_$(echo "$VERSION" | tr . _)" "$FPM_SOCKET"
  pkg_env "BIN_$(echo "$VERSION" | tr . _)" "/usr/bin/php$VERSION"
}
pkg_uninstall() {
  svc disable "$SERVICE"
  case "$(os_family)" in
    rpm) os_remove "php$V2-php-*" "php$V2-runtime"; rm -f "/usr/bin/php$VERSION" ;;
    *) os_remove "php$VERSION-*" ;;
  esac
}
