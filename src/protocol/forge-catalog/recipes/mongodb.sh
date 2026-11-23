SERVICE=mongod
PROC=/usr/bin/mongod
OS_PKG=mongodb-org-server
UPGRADE_WITH="mongodb-org mongodb-org-database mongodb-org-mongos mongodb-org-tools mongodb-org-database-tools-extra"
pkg_running() { mongod --version | sed -n 's/^db version v//p'; }
pkg_install() {
  need_supported
  case "$(os_family)" in
    deb)
      case "$OS_ID" in
        *ubuntu*) line="https://repo.mongodb.org/apt/ubuntu $CODENAME/mongodb-org/$VERSION multiverse" ;;
        *) line="https://repo.mongodb.org/apt/debian $CODENAME/mongodb-org/$VERSION main" ;;
      esac
      apt_repo mongodb "https://pgp.mongodb.com/server-$VERSION.asc" "$line"
      ;;
    rpm)
      rpm_repo mongodb "https://repo.mongodb.org/yum/redhat/$EL_MAJOR/mongodb-org/$VERSION/$RPM_ARCH/" \
        "https://pgp.mongodb.com/server-$VERSION.asc"
      ;;
  esac
  if [ -n "$BUILD" ]; then
    # Keep server, shell and tools on the same build.
    os_install "$(pinned mongodb-org-server)" "$(pinned mongodb-org-mongos)" mongodb-mongosh mongodb-database-tools
  else
    os_install mongodb-org
  fi
  pw=$(pkg_secret PASSWORD)
  sed -i "s/^\( *port:\).*/\1 $FORGE_P_PORT/; s/^\( *bindIp:\).*/\1 $(bind_addr)/" /etc/mongod.conf
  [ "$FORGE_P_PORT" = 27017 ] || selinux_port mongod_port_t "$FORGE_P_PORT"
  fw_for_bind "$FORGE_P_PORT"
  svc enable mongod
  svc restart mongod
  i=0
  until mongosh --quiet --port "$FORGE_P_PORT" --eval 'db.runCommand({ping:1})' >/dev/null 2>&1; do
    i=$((i + 1)); [ "$i" -lt 60 ] || die "mongod did not start"; sleep 1
  done
  mongosh --quiet --port "$FORGE_P_PORT" admin \
    --eval "if (!db.getUser('forge_admin')) db.createUser({user:'forge_admin', pwd:'$pw', roles:['root']})" >/dev/null 2>&1 || true
  grep -q '^security:' /etc/mongod.conf || printf 'security:\n  authorization: enabled\n' >> /etc/mongod.conf
  svc restart mongod
  pkg_env PORT "$FORGE_P_PORT"
  pkg_env USER forge_admin
}
pkg_uninstall() {
  svc disable mongod
  fw_close "$FORGE_P_PORT"
  os_remove 'mongodb-org*'
}
