# Jenkins LTS from pkg.jenkins.io, on the distribution's Java 17/21.
OS_PKG=jenkins
SERVICE=jenkins
PROC=jenkins.war
pkg_running() { jenkins --version 2>/dev/null | head -1; }
pkg_install() {
  need_supported
  need_systemd
  port_free "$FORGE_P_PORT"
  case "$(os_family)" in
    deb)
      apt_repo jenkins https://pkg.jenkins.io/debian-stable/jenkins.io-2023.key "https://pkg.jenkins.io/debian-stable binary/"
      apt_update
      if [ -n "$(os_candidate openjdk-21-jre-headless)" ]; then jre=openjdk-21-jre-headless; else jre=openjdk-17-jre-headless; fi
      os_install fontconfig "$jre"
      ;;
    rpm)
      rpm_repo jenkins https://pkg.jenkins.io/redhat-stable https://pkg.jenkins.io/redhat-stable/jenkins.io-2023.key
      os_install fontconfig java-21-openjdk-headless
      ;;
  esac
  os_install "$(pinned jenkins)"
  install -d -m 0755 /etc/systemd/system/jenkins.service.d
  {
    echo "# Managed by Orbyn Forge"
    echo "[Service]"
    echo "Environment=\"JENKINS_PORT=$FORGE_P_PORT\""
    echo "Environment=\"JENKINS_LISTEN_ADDRESS=$(bind_addr)\""
  } > /etc/systemd/system/jenkins.service.d/90-forge.conf
  systemctl daemon-reload
  selinux_port http_port_t "$FORGE_P_PORT"
  fw_for_bind "$FORGE_P_PORT"
  svc enable jenkins
  svc restart jenkins
  wait_http "http://127.0.0.1:$FORGE_P_PORT/login" 240
  pkg_env URL "http://127.0.0.1:$FORGE_P_PORT"
  # The setup wizard's one-time password stays on the node.
  pkg_env INITIAL_ADMIN_PASSWORD_FILE /var/lib/jenkins/secrets/initialAdminPassword
}
pkg_uninstall() {
  svc disable jenkins
  fw_close "$FORGE_P_PORT"
  rm -rf /etc/systemd/system/jenkins.service.d/90-forge.conf
  os_remove jenkins
}
