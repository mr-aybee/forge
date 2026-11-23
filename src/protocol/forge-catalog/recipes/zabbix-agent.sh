# Zabbix agent 2 from repo.zabbix.com (release track, e.g. 7.0 LTS),
# reporting to your Zabbix server.
OS_PKG=zabbix-agent2
SERVICE=zabbix-agent2
PROC=/usr/sbin/zabbix_agent2
pkg_running() { zabbix_agent2 -V 2>/dev/null | sed -n 's/.*(Zabbix) \([0-9.]*\).*/\1/p' | head -1; }
pkg_install() {
  need_supported
  [ -n "$FORGE_P_SERVER" ] || die "the Zabbix server address is required"
  if [ -z "$(os_installed zabbix-release)" ]; then
    case "$(os_family)" in
      deb)
        case "$OS_ID" in
          *ubuntu*) rel="ubuntu${VERSION_ID}" ; path=ubuntu ;;
          *) rel="debian${VERSION_ID%%.*}" ; path=debian ;;
        esac
        fetch "https://repo.zabbix.com/zabbix/$VERSION/$path/pool/main/z/zabbix-release/zabbix-release_latest_$VERSION+${rel}_all.deb" /tmp/forge-zabbix-release.deb
        dpkg -i /tmp/forge-zabbix-release.deb >/dev/null; rm -f /tmp/forge-zabbix-release.deb
        APT_UPDATED=""
        ;;
      rpm)
        # EPEL ships older Zabbix packages that would shadow the official ones.
        if [ -f /etc/yum.repos.d/epel.repo ] && ! grep -q '^excludepkgs=zabbix\*' /etc/yum.repos.d/epel.repo; then
          sed -i '/^\[epel\]/a excludepkgs=zabbix*' /etc/yum.repos.d/epel.repo
        fi
        "$DNF_BIN" install -y -q "https://repo.zabbix.com/zabbix/$VERSION/rhel/$EL_MAJOR/$RPM_ARCH/zabbix-release-latest-$VERSION.el$EL_MAJOR.noarch.rpm" >/dev/null
        ;;
    esac
  fi
  os_install "$(pinned zabbix-agent2)"
  conf=/etc/zabbix/zabbix_agent2.conf
  host="$(hostname)"
  sed -i "s/^Server=.*/Server=$FORGE_P_SERVER/; s/^ServerActive=.*/ServerActive=$FORGE_P_SERVER/; s/^Hostname=.*/Hostname=$host/" "$conf"
  fw_open 10050
  svc enable zabbix-agent2
  svc restart zabbix-agent2
  pkg_env SERVER "$FORGE_P_SERVER"
  pkg_env HOSTNAME "$host"
}
pkg_uninstall() { svc disable zabbix-agent2; fw_close 10050; os_remove zabbix-agent2; }
