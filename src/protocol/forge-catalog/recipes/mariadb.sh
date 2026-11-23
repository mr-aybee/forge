SERVICE=mariadb
PROC=mariadbd
OS_PKG=mariadb-server
UPGRADE_WITH="mariadb-client mariadb-common mariadb-server-core mariadb-client-core mariadb mariadb-errmsg mariadb-server-utils mariadb-backup mariadb-gssapi-server"
pkg_running() { mysql -N -e 'SELECT VERSION()' 2>/dev/null | sed 's/-.*//'; }
pkg_install() {
  need_supported
  os_install "$(pinned mariadb-server)"
  case "$(os_family)" in
    rpm) cnf=/etc/my.cnf.d/90-forge.cnf ;;
    *) cnf=/etc/mysql/mariadb.conf.d/90-forge.cnf ;;
  esac
  printf '[mysqld]\nport = %s\nbind-address = %s\n' "$FORGE_P_PORT" "$(bind_addr)" > "$cnf"
  [ "$FORGE_P_PORT" = 3306 ] || selinux_port mysqld_port_t "$FORGE_P_PORT"
  fw_for_bind "$FORGE_P_PORT"
  svc enable mariadb
  svc restart mariadb
  pw=$(pkg_secret PASSWORD)
  i=0
  until mysqladmin ping >/dev/null 2>&1; do i=$((i + 1)); [ "$i" -lt 30 ] || die "mariadb did not start"; sleep 1; done
  mysql -e "CREATE USER IF NOT EXISTS 'forge_admin'@'%' IDENTIFIED BY '$pw'; ALTER USER 'forge_admin'@'%' IDENTIFIED BY '$pw'; GRANT ALL ON *.* TO 'forge_admin'@'%' WITH GRANT OPTION; FLUSH PRIVILEGES;"
  pkg_env HOST 127.0.0.1
  pkg_env PORT "$FORGE_P_PORT"
  pkg_env USER forge_admin
}
pkg_uninstall() {
  svc disable mariadb
  fw_close "$FORGE_P_PORT"
  case "$(os_family)" in
    rpm) os_remove mariadb-server; rm -f /etc/my.cnf.d/90-forge.cnf ;;
    *) os_remove mariadb-server mariadb-common; rm -f /etc/mysql/mariadb.conf.d/90-forge.cnf ;;
  esac
}
