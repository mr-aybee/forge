OS_PKG="temurin-$VERSION-jdk"
case "$(os_family)" in
  rpm) JHOME="/usr/lib/jvm/temurin-$VERSION-jdk" ;;
  *) JHOME="/usr/lib/jvm/temurin-$VERSION-jdk-$ARCH" ;;
esac
pkg_running() { "$JHOME/bin/java" -version 2>&1 | sed -n 's/.*version "\([^"]*\)".*/\1/p' | head -1; }
pkg_install() {
  need_supported
  case "$(os_family)" in
    deb)
      apt_repo adoptium https://packages.adoptium.net/artifactory/api/gpg/key/public "https://packages.adoptium.net/artifactory/deb $CODENAME main"
      ;;
    rpm)
      rpm_repo adoptium "https://packages.adoptium.net/artifactory/rpm/rhel/$EL_MAJOR/$RPM_ARCH" \
        https://packages.adoptium.net/artifactory/api/gpg/key/public
      ;;
  esac
  os_install "$(pinned "$OS_PKG")"
  [ -x "$JHOME/bin/java" ] || JHOME="$(dirname "$(dirname "$(readlink -f "$(command -v java)")")")"
  pkg_env HOME "$JHOME"
  pkg_env BIN "$JHOME/bin/java"
}
pkg_uninstall() { os_remove "$OS_PKG"; }
pkg_status() { [ -x "$JHOME/bin/java" ] && echo running || echo not-installed; }
