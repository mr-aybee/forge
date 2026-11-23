SERVICE=nginx
PROC="nginx: master"
OS_PKG=nginx
UPGRADE_WITH="nginx-common nginx-core nginx-filesystem"
pkg_running() { nginx -v 2>&1 | sed -n 's|.*nginx/\([^ ]*\).*|\1|p'; }
# A reload keeps connections open; nginx reloads itself on package upgrade.
pkg_after_upgrade() { nginx -t >/dev/null 2>&1 && svc reload nginx 2>/dev/null || svc restart nginx; }
pkg_install() {
  need_supported
  os_install "$(pinned nginx)"
  install -d -m 0755 /etc/nginx/forge/sites /etc/nginx/forge/custom
  if ! grep -qs 'include /etc/nginx/forge/sites/\*.conf;' /etc/nginx/nginx.conf; then
    if grep -qs 'include /etc/nginx/sites-enabled/\*;' /etc/nginx/nginx.conf; then
      # Debian/Ubuntu layout.
      sed -i 's|^\(\s*\)include /etc/nginx/sites-enabled/\*;|&\n\1include /etc/nginx/forge/sites/*.conf;|' /etc/nginx/nginx.conf
    else
      # RHEL / nginx.org layout: next to conf.d inside http {}.
      sed -i 's|^\(\s*\)include /etc/nginx/conf.d/\*.conf;|&\n\1include /etc/nginx/forge/sites/*.conf;|' /etc/nginx/nginx.conf
    fi
  fi
  grep -qs 'include /etc/nginx/forge/sites/\*.conf;' /etc/nginx/nginx.conf || die "could not add Forge sites to /etc/nginx/nginx.conf"
  # SELinux: let Nginx proxy to upstream services.
  if selinux_on; then setsebool -P httpd_can_network_connect 1 >/dev/null 2>&1 || true; fi
  fw_open 80
  fw_open 443
  nginx -t >/dev/null 2>&1 || die "nginx configuration test failed"
  svc enable nginx
  pkg_env SITES_DIR /etc/nginx/forge/sites
  pkg_env BIN /usr/sbin/nginx
}
pkg_uninstall() {
  svc disable nginx
  case "$(os_family)" in rpm) os_remove nginx nginx-core nginx-filesystem ;; *) os_remove nginx nginx-common nginx-core ;; esac
}
