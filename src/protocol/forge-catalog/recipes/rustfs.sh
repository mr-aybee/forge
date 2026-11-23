CONTAINER=forge-rustfs
pkg_install() {
  need_docker
  ak=$(pkg_secret ACCESS_KEY)
  sk=$(pkg_secret SECRET_KEY)
  install -d -m 0777 "$PKG_DATA"
  docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  step docker run -d --name "$CONTAINER" "rustfs/rustfs:$VERSION"
  docker run -d --name "$CONTAINER" --restart unless-stopped \
    -e "RUSTFS_ACCESS_KEY=$ak" -e "RUSTFS_SECRET_KEY=$sk" -e RUSTFS_CONSOLE_ENABLE=true \
    -p "$FORGE_P_PORT:9000" -p "$FORGE_P_CONSOLE_PORT:9001" -v "$PKG_DATA:/data" \
    "rustfs/rustfs:$VERSION" >/dev/null
  pkg_env ENDPOINT "http://127.0.0.1:$FORGE_P_PORT"
}
pkg_uninstall() { docker rm -f "$CONTAINER" >/dev/null 2>&1 || true; }
