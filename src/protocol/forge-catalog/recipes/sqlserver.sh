CONTAINER=forge-sqlserver
pkg_install() {
  need_docker
  [ "$FORGE_P_ACCEPT_EULA" = true ] || die "the SQL Server EULA must be accepted"
  pw="Aa1!$(pkg_secret SA_PASSWORD_SEED)"
  pkg_env SA_PASSWORD "$pw"
  install -d -m 0777 "$PKG_DATA"
  docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  step docker run -d --name "$CONTAINER"
  docker run -d --name "$CONTAINER" --restart unless-stopped \
    -e ACCEPT_EULA=Y -e "MSSQL_PID=$FORGE_P_EDITION" -e "MSSQL_SA_PASSWORD=$pw" \
    -p "$FORGE_P_PORT:1433" -v "$PKG_DATA:/var/opt/mssql" \
    "mcr.microsoft.com/mssql/server:$VERSION-latest" >/dev/null
  pkg_env HOST 127.0.0.1
  pkg_env PORT "$FORGE_P_PORT"
  pkg_env USER sa
}
pkg_uninstall() { docker rm -f "$CONTAINER" >/dev/null 2>&1 || true; }
