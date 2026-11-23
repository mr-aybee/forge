# Apache Tomcat (distribution package: tomcat10 on Debian/Ubuntu, tomcat 9
# on RHEL 9), with the distribution's Java.
case "$(os_family)" in
  rpm) OS_PKG=tomcat; SERVICE=tomcat; TC=/usr/share/tomcat; TCCONF=/etc/tomcat; UPGRADE_WITH="tomcat-lib tomcat-el-3.0-api tomcat-jsp-2.3-api tomcat-servlet-4.0-api" ;;
  *) OS_PKG=tomcat10; SERVICE=tomcat10; TC=/usr/share/tomcat10; TCCONF=/etc/tomcat10; UPGRADE_WITH="tomcat10-common libtomcat10-java" ;;
esac
PROC=org.apache.catalina.startup.Bootstrap
pkg_running() { java -cp "$TC/lib/catalina.jar" org.apache.catalina.util.ServerInfo 2>/dev/null | sed -n 's/^Server number: *//p'; }
pkg_install() {
  need_supported
  port_free "$FORGE_P_PORT"
  os_install "$(pinned "$OS_PKG")"
  # The HTTP connector: <Connector port="8080" protocol="HTTP/1.1" …
  sed -i "s/<Connector port=\"[0-9]*\" protocol=\"HTTP\/1.1\"/<Connector port=\"$FORGE_P_PORT\" protocol=\"HTTP\/1.1\"/" "$TCCONF/server.xml"
  if [ "$FORGE_P_BIND" != all ] && ! grep -q 'address="127.0.0.1" protocol="HTTP/1.1"' "$TCCONF/server.xml"; then
    sed -i "s/<Connector port=\"$FORGE_P_PORT\" protocol=\"HTTP\/1.1\"/<Connector port=\"$FORGE_P_PORT\" address=\"127.0.0.1\" protocol=\"HTTP\/1.1\"/" "$TCCONF/server.xml"
  fi
  [ "$FORGE_P_PORT" = 8080 ] || selinux_port http_cache_port_t "$FORGE_P_PORT"
  fw_for_bind "$FORGE_P_PORT"
  svc enable "$SERVICE"
  svc restart "$SERVICE"
  wait_http "http://127.0.0.1:$FORGE_P_PORT/" 90
  pkg_env PORT "$FORGE_P_PORT"
  pkg_env WEBAPPS "$( [ "$(os_family)" = rpm ] && echo /var/lib/tomcat/webapps || echo /var/lib/tomcat10/webapps )"
}
pkg_uninstall() { svc disable "$SERVICE"; fw_close "$FORGE_P_PORT"; os_remove "$OS_PKG"; }
