OS_PKG=certbot
UPGRADE_WITH="python3-certbot python3-certbot-nginx"
pkg_running() { certbot --version 2>&1 | awk '{print $2}'; }
pkg_after_upgrade() { :; }
pkg_install() {
  need_supported
  command -v nginx >/dev/null 2>&1 || die "install Nginx on this node first"
  if [ "$(os_family)" = rpm ]; then epel_enable; fi
  os_install "$(pinned certbot)" python3-certbot-nginx
  # Renewals: the distro timer runs `certbot renew` twice a day; Nginx must
  # reload to serve the renewed certificate.
  mkdir -p /etc/letsencrypt/renewal-hooks/deploy
  cat > /etc/letsencrypt/renewal-hooks/deploy/forge-nginx-reload.sh <<'HOOK'
#!/bin/sh
# Installed by Orbyn Forge: serve renewed certificates.
nginx -t && (systemctl reload nginx || nginx -s reload)
HOOK
  chmod 0755 /etc/letsencrypt/renewal-hooks/deploy/forge-nginx-reload.sh
  # Debian/Ubuntu: certbot.timer; RHEL (EPEL): certbot-renew.timer.
  systemctl enable --now certbot.timer >/dev/null 2>&1 || systemctl enable --now certbot-renew.timer >/dev/null 2>&1 || true
  pkg_env BIN /usr/bin/certbot
}
pkg_uninstall() {
  rm -f /etc/letsencrypt/renewal-hooks/deploy/forge-nginx-reload.sh
  os_remove certbot python3-certbot-nginx
}
pkg_status() { command -v certbot >/dev/null && echo running || echo not-installed; }
