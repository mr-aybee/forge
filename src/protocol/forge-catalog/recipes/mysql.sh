# MySQL Community from Oracle's repositories (repo.mysql.com), one release
# track per install: 8.4 (LTS) or 8.0.
OS_PKG=mysql-community-server
UPGRADE_WITH="mysql-community-client mysql-community-server-core mysql-community-client-core mysql-common mysql-community-client-plugins mysql-community-common mysql-community-libs mysql-community-icu-data-files"
case "$(os_family)" in
  rpm) SERVICE=mysqld; CNF=/etc/my.cnf.d/90-forge.cnf ;;
  *) SERVICE=mysql; CNF=/etc/mysql/mysql.conf.d/90-forge.cnf ;;
esac
PROC=mysqld
TRACK="mysql-$VERSION"; [ "$VERSION" = 8.4 ] && TRACK="mysql-8.4-lts"
pkg_running() { mysqld --version 2>/dev/null | sed -n 's/.*Ver \([0-9.]*\).*/\1/p'; }
pkg_install() {
  need_supported
  port_free "$FORGE_P_PORT"
  if [ -n "$(os_installed mariadb-server)" ]; then die "MariaDB is installed on this node; MySQL and MariaDB cannot share a node"; fi
  case "$(os_family)" in
    deb)
      case "$OS_ID" in *ubuntu*) dist=ubuntu ;; *) dist=debian ;; esac
      apt_repo mysql https://repo.mysql.com/RPM-GPG-KEY-mysql-2025 "http://repo.mysql.com/apt/$dist $CODENAME $TRACK"
      ;;
    rpm)
      rpm_repo mysql "https://repo.mysql.com/yum/$(echo "$TRACK" | sed 's/^mysql-8.4-lts$/mysql-8.4-community/; s/^mysql-8.0$/mysql-8.0-community/')/el/$EL_MAJOR/$RPM_ARCH/" \
        https://repo.mysql.com/RPM-GPG-KEY-mysql-2025
      "$DNF_BIN" -qy module disable mysql >/dev/null 2>&1 || true
      ;;
  esac
  os_install "$(pinned mysql-community-server)"
  install -d -m 0755 "$(dirname "$CNF")"
  printf '[mysqld]\nport = %s\nbind-address = %s\n' "$FORGE_P_PORT" "$(bind_addr)" > "$CNF"
  if [ "$(os_family)" = rpm ] && ! grep -q '^!includedir /etc/my.cnf.d' /etc/my.cnf 2>/dev/null; then
    echo '!includedir /etc/my.cnf.d' >> /etc/my.cnf
  fi
  [ "$FORGE_P_PORT" = 3306 ] || selinux_port mysqld_port_t "$FORGE_P_PORT"
  fw_for_bind "$FORGE_P_PORT"
  svc enable "$SERVICE"
  svc restart "$SERVICE"
  pw=$(pkg_secret PASSWORD)
  i=0
  until mysqladmin --protocol=socket ping >/dev/null 2>&1; do i=$((i + 1)); [ "$i" -lt 60 ] || die "mysqld did not start"; sleep 1; done
  sql="CREATE USER IF NOT EXISTS 'forge_admin'@'%' IDENTIFIED BY '$pw'; ALTER USER 'forge_admin'@'%' IDENTIFIED BY '$pw'; GRANT ALL ON *.* TO 'forge_admin'@'%' WITH GRANT OPTION; FLUSH PRIVILEGES;"
  if ! mysql -e "$sql" 2>/dev/null; then
    # RHEL packages give root a temporary password on first start.
    tmp="$(sed -n 's/.*temporary password is generated for root@localhost: //p' /var/log/mysqld.log 2>/dev/null | tail -1)"
    [ -n "$tmp" ] || die "cannot log in to MySQL as root"
    rootpw=$(pkg_secret ROOT_PASSWORD)
    mysql --connect-expired-password -uroot -p"$tmp" -e "ALTER USER 'root'@'localhost' IDENTIFIED BY '$rootpw';" 2>/dev/null
    mysql -uroot -p"$rootpw" -e "$sql" 2>/dev/null || die "cannot create the forge_admin user"
  fi
  pkg_env HOST 127.0.0.1
  pkg_env PORT "$FORGE_P_PORT"
  pkg_env USER forge_admin
}
pkg_uninstall() {
  svc disable "$SERVICE"
  fw_close "$FORGE_P_PORT"
  os_remove mysql-community-server mysql-community-client mysql-common
  rm -f "$CNF"
}
