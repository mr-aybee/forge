case "$(os_family)" in
  rpm)
    OS_PKG="postgresql$VERSION-server"
    UPGRADE_WITH="postgresql$VERSION postgresql$VERSION-libs postgresql$VERSION-contrib"
    SERVICE="postgresql-$VERSION"
    PGBIN="/usr/pgsql-$VERSION/bin"
    PGCONF="/var/lib/pgsql/$VERSION/data"
    ;;
  *)
    OS_PKG="postgresql-$VERSION"
    UPGRADE_WITH="postgresql-client-$VERSION"
    SERVICE=postgresql
    PGBIN="/usr/lib/postgresql/$VERSION/bin"
    PGCONF="/etc/postgresql/$VERSION/main"
    ;;
esac
PROC="$PGBIN/postgres"
pg() { su postgres -c "$PGBIN/psql -p ${FORGE_P_PORT:-5432} $1"; }
pkg_running() { pg "-qtAc 'show server_version'"; }
pkg_install() {
  need_supported
  if [ -z "$(os_installed "$OS_PKG")" ]; then
    case "$(os_family)" in
      deb)
        apt_update
        if [ -z "$(os_candidate "$OS_PKG")" ]; then
          apt_repo pgdg https://www.postgresql.org/media/keys/ACCC4CF8.asc "https://apt.postgresql.org/pub/repos/apt $CODENAME-pgdg main"
        fi
        ;;
      rpm)
        if [ -z "$(os_candidate "$OS_PKG")" ]; then
          "$DNF_BIN" install -y -q "https://download.postgresql.org/pub/repos/yum/reporpms/EL-$EL_MAJOR-$RPM_ARCH/pgdg-redhat-repo-latest.noarch.rpm" >/dev/null
          # The distribution's own PostgreSQL module would shadow PGDG.
          "$DNF_BIN" -qy module disable postgresql >/dev/null 2>&1 || true
        fi
        ;;
    esac
    os_install "$(pinned "$OS_PKG")"
  fi
  if [ "$(os_family)" = rpm ] && [ ! -f "$PGCONF/PG_VERSION" ]; then
    PGSETUP_INITDB_OPTIONS="--auth-host=scram-sha-256 --auth-local=peer" "$PGBIN/postgresql-$VERSION-setup" initdb >/dev/null
  fi
  sed -i "s/^#\?port = [0-9]*/port = $FORGE_P_PORT/" "$PGCONF/postgresql.conf"
  addr=localhost
  [ "$FORGE_P_BIND" = all ] && addr='*'
  sed -i "s/^#\?listen_addresses = '[^']*'/listen_addresses = '$addr'/" "$PGCONF/postgresql.conf"
  if [ "$FORGE_P_BIND" = all ] && ! grep -q 'forge-managed' "$PGCONF/pg_hba.conf"; then
    printf '# forge-managed\nhost all all 0.0.0.0/0 scram-sha-256\nhost all all ::/0 scram-sha-256\n' >> "$PGCONF/pg_hba.conf"
  fi
  [ "$FORGE_P_PORT" = 5432 ] || selinux_port postgresql_port_t "$FORGE_P_PORT"
  fw_for_bind "$FORGE_P_PORT"
  svc enable "$SERVICE"
  svc restart "$SERVICE"
  pw=$(pkg_secret PASSWORD)
  i=0
  until pg "-qtAc 'select 1'" >/dev/null 2>&1; do
    i=$((i + 1)); [ "$i" -lt 30 ] || die "postgres did not start"; sleep 1
  done
  pg "-qc \"ALTER ROLE postgres PASSWORD '$pw'\"" >/dev/null
  pkg_env HOST 127.0.0.1
  pkg_env PORT "$FORGE_P_PORT"
  pkg_env USER postgres
  pkg_env URL "postgres://postgres:$pw@127.0.0.1:$FORGE_P_PORT/postgres"
}
pkg_uninstall() {
  svc disable "$SERVICE"
  fw_close "$FORGE_P_PORT"
  case "$(os_family)" in
    rpm) os_remove "$OS_PKG" "postgresql$VERSION" ;;
    *) os_remove "$OS_PKG" postgresql-common ;;
  esac
}
