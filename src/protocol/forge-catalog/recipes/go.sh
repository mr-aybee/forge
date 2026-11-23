pkg_install() {
  command -v curl >/dev/null 2>&1 || os_install ca-certificates curl
  tarball="go$VERSION.linux-$GOARCH.tar.gz"
  fetch "https://go.dev/dl/$tarball" "/tmp/$tarball"
  want=$(curl -fsSL "https://go.dev/dl/?mode=json&include=all" | tr '{' '\n' \
    | grep "\"filename\": *\"$tarball\"" | sed -n 's/.*"sha256": *"\([0-9a-f]*\)".*/\1/p' | head -1)
  [ -n "$want" ] || die "could not read the checksum of $tarball from go.dev"
  echo "$want  /tmp/$tarball" | sha256sum -c - >/dev/null || die "checksum mismatch for $tarball"
  rm -rf "$PKG_HOME"
  install -d -m 0755 "$PKG_HOME"
  tar -C "$PKG_HOME" --strip-components=1 -xzf "/tmp/$tarball"
  rm -f "/tmp/$tarball"
  ln -sf "$PKG_HOME/bin/go" /usr/local/bin/go
  pkg_env HOME "$PKG_HOME"
  pkg_env BIN "$PKG_HOME/bin/go"
}
pkg_uninstall() { rm -rf "$PKG_HOME"; rm -f /usr/local/bin/go; }
pkg_status() { [ -x "$PKG_HOME/bin/go" ] && echo running || echo not-installed; }
