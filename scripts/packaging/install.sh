#!/bin/sh
# Installs (or upgrades) the Orbyn Forge control plane from this package.
#
#   sudo ./install.sh --public-url http://forge.example.com:8080
#
# Options:
#   --public-url URL        URL users open (default: http://<hostname>:8080)
#   --public-address URL    address people use when Forge is published on a
#                           domain through an ingress (https://forge.example.com,
#                           origin only). Stored as the runtime setting; links in
#                           alerts use it. Change it later in the console
#                           (Settings) or with `forge-server public-address set`.
#   --trusted-proxies LIST  comma-separated IPs/CIDRs of reverse proxies (the
#                           ingress edges) whose X-Forwarded-For/-Proto Forge
#                           trusts; new configuration only (default: none)
#   --admin-username NAME   first platform administrator (first install only;
#                           default: admin)
#   --admin-email EMAIL     optional email for that administrator; it can also
#                           be added later in the console
#   --agent-host NAME       name/IP nodes use to reach this server (default: from --public-url)
#   --prefix DIR            binaries (default /opt/forge)
#   --environment ENV       production (default), staging, development or test
#   --mfa required|off      whether guarded actions also need MFA (default on a
#                           new install: off, MFA is each user's choice for their
#                           own sign-in; an upgrade keeps the setting).
#                           Change it later with `forge-server mfa on|off`.
#   --dev-skip-mfa          deprecated: the same as --mfa off
#   --allow-elevated-tokens yes|no
#                           whether elevated API tokens (CI) may skip the MFA
#                           prompt (default on a new install: no for production
#                           with MFA required, otherwise yes; an upgrade keeps it)
#   --install-agent         also install the Forge agent on this server and
#                           enroll it, so it runs workloads too
#   --cluster NAME          cluster for that agent (default: default; created
#                           if missing)
#   --terminal-account NAME terminal account for that agent (default: forge-ops)
#   --db sqlite|postgres    operational database (default sqlite)
#   --db-url URL            PostgreSQL server without database and password,
#                           e.g. postgres://forge@db1:5432 (password in
#                           FORGE_DATABASE_PASSWORD)
#   --db-local              install PostgreSQL on this machine and use it
#   --db-name NAME          database name (default forge_op; created when
#                           missing, reused when it exists)
#   --allow-shared-db       use a database that also holds other tables
#   --analytics duckdb|chdb|clickhouse|off   (default duckdb; embedded = chdb)
#                           analytics database for node history and logs;
#                           new configuration only
#   --analytics-url URL     ClickHouse server for --analytics clickhouse
#                           (password in FORGE_ANALYTICS_PASSWORD)
#   --analytics-name NAME   default: <db-name>_analytics
#   --yes, --non-interactive
#                           never ask questions
#   --uninstall             stop and remove the service and the binaries;
#                           keeps configuration and data (add --purge to
#                           delete them and the forge user too)
#
# Interactive mode: on a terminal (stdin and stdout), without --yes and without
# any option that chooses a setting, the installer asks for each setting, shows
# a review screen and installs only after you confirm.
#
# On a first install the administrator's password is generated, printed once
# and saved to /var/lib/forge/initial-admin-password (root only). Forge deletes
# that file when the administrator changes the password. Set
# FORGE_ADMIN_PASSWORD to choose the password instead (nothing is saved then).
#
# Layout: binaries in /opt/forge (forge-server, and forge-analytics-worker and
# forge-plugin-runner, which forge-server starts and supervises), agent
# binaries in /opt/forge/dist, configuration /etc/forge/forge.toml, data
# /var/lib/forge, service forge-server.service (user `forge`). Re-running
# replaces the binaries and keeps configuration and data. Nothing else needs
# to be installed: the server, the console and the agent binaries are here.
# Before changing anything, the installer checks the package against its own
# checksums (dist/SHA256SUMS) and this machine's architecture; get-forge.sh
# checks the package's signature before it runs this script.
set -eu

PREFIX=/opt/forge
CONF_DIR=/etc/forge
DATA_DIR=/var/lib/forge
PUBLIC_URL=""
PUBLIC_ADDRESS=""
TRUSTED_PROXIES=""
ADMIN_USER=""
ADMIN_EMAIL=""
AGENT_HOST=""
ENVIRONMENT=""
MFA=""
ALLOW_ELEVATED=""
INSTALL_AGENT=false
CLUSTER=""
TERMINAL_ACCOUNT=""
ASSUME_YES=false
SETTINGS_GIVEN=false
DB_KIND=""
DB_URL=""
DB_LOCAL=false
DB_NAME=""
ALLOW_SHARED_DB=false
ANALYTICS=""
ANALYTICS_URL=""
ANALYTICS_NAME=""
NEW_ELEVATED_DEFAULT=""
UNINSTALL=false
PURGE=false

while [ $# -gt 0 ]; do
  case "$1" in
    --public-url) PUBLIC_URL="$2"; SETTINGS_GIVEN=true; shift 2 ;;
    --public-address) PUBLIC_ADDRESS="$2"; SETTINGS_GIVEN=true; shift 2 ;;
    --trusted-proxies) TRUSTED_PROXIES="$2"; SETTINGS_GIVEN=true; shift 2 ;;
    --admin-username) ADMIN_USER="$2"; SETTINGS_GIVEN=true; shift 2 ;;
    --admin-email) ADMIN_EMAIL="$2"; SETTINGS_GIVEN=true; shift 2 ;;
    --agent-host) AGENT_HOST="$2"; SETTINGS_GIVEN=true; shift 2 ;;
    --prefix) PREFIX="$2"; shift 2 ;;
    --environment) ENVIRONMENT="$2"; SETTINGS_GIVEN=true; shift 2 ;;
    --mfa) MFA="$2"; SETTINGS_GIVEN=true; shift 2 ;;
    --dev-skip-mfa) MFA=off; SETTINGS_GIVEN=true; shift ;;
    --allow-elevated-tokens) ALLOW_ELEVATED="$2"; SETTINGS_GIVEN=true; shift 2 ;;
    --install-agent) INSTALL_AGENT=true; SETTINGS_GIVEN=true; shift ;;
    --cluster) CLUSTER="$2"; SETTINGS_GIVEN=true; shift 2 ;;
    --terminal-account) TERMINAL_ACCOUNT="$2"; SETTINGS_GIVEN=true; shift 2 ;;
    --db) DB_KIND="$2"; SETTINGS_GIVEN=true; shift 2 ;;
    --db-url) DB_URL="$2"; DB_KIND=postgres; SETTINGS_GIVEN=true; shift 2 ;;
    --db-local) DB_LOCAL=true; DB_KIND=postgres; SETTINGS_GIVEN=true; shift ;;
    --db-name) DB_NAME="$2"; SETTINGS_GIVEN=true; shift 2 ;;
    --allow-shared-db) ALLOW_SHARED_DB=true; shift ;;
    --analytics) ANALYTICS="$2"; SETTINGS_GIVEN=true; shift 2 ;;
    --analytics-url) ANALYTICS_URL="$2"; SETTINGS_GIVEN=true; shift 2 ;;
    --analytics-name) ANALYTICS_NAME="$2"; SETTINGS_GIVEN=true; shift 2 ;;
    --yes|--non-interactive) ASSUME_YES=true; shift ;;
    --uninstall) UNINSTALL=true; shift ;;
    --purge) PURGE=true; shift ;;
    -h|--help) awk 'NR > 1 && /^set -eu$/ { exit } NR > 1' "$0"; exit 0 ;;
    *) echo "unknown option $1 (see --help)" >&2; exit 2 ;;
  esac
done
[ -n "$ADMIN_USER" ] || ADMIN_USER=admin
[ -n "$CLUSTER" ] || CLUSTER=default
[ -n "$TERMINAL_ACCOUNT" ] || TERMINAL_ACCOUNT=forge-ops

# ------------------------------------------------------------------ output
# Colour only on a terminal and unless NO_COLOR is set; box-drawing and tick
# characters only in a UTF-8 locale.
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
  B=$(printf '\033[1m'); D=$(printf '\033[2m'); G=$(printf '\033[32m'); R=$(printf '\033[31m')
  Y=$(printf '\033[33m'); C=$(printf '\033[36m'); N=$(printf '\033[0m')
else
  B=""; D=""; G=""; R=""; Y=""; C=""; N=""
fi
case "$(locale charmap 2>/dev/null || echo)" in
  UTF-8) OK="✔"; BAD="✖"; WARN="▲"; DOT="·"; TL="╭"; BL="╰"; H="─"; V="│" ;;
  *) OK="+"; BAD="x"; WARN="!"; DOT="-"; TL="+"; BL="+"; H="-"; V="|" ;;
esac

rule() { i=0; while [ "$i" -lt "$1" ]; do printf '%s' "$H"; i=$((i + 1)); done; }
banner() { # banner <title> <subtitle>  (open on the right: no padding of multi-byte text)
  printf '\n  %s%s%s%s %s%s%s\n' "$C" "$TL" "$H" "$N" "$B" "$1" "$N"
  printf '  %s%s%s  %s%s%s\n' "$C" "$V" "$N" "$D" "$2" "$N"
  printf '  %s%s%s%s\n\n' "$C" "$BL" "$(rule 60)" "$N"
}
step() { printf '  %s%s%s %-22s %s%s%s\n' "$G" "$OK" "$N" "$1" "$D" "$2" "$N"; }
note() { printf '  %s%s%s %-22s %s\n' "$Y" "$WARN" "$N" "$1" "$2"; }
section() { printf '\n  %s%s%s\n' "$B" "$1" "$N"; }
row() { printf '    %-13s %s\n' "$1" "$2"; }
die() { printf '\n  %s%s%s %s\n\n' "$R" "$BAD" "$N" "$*" >&2; exit 1; }

case "${ENVIRONMENT:-production}" in
  production|staging|development|test) ;;
  *) die "--environment must be production, staging, development or test (got $ENVIRONMENT)." ;;
esac
case "$MFA" in
  ""|required|off) ;;
  on) MFA=required ;;
  *) die "--mfa must be required or off (got $MFA)." ;;
esac
norm_yes_no() { # prints true/false, or nothing for an invalid answer
  case "$1" in
    y|Y|yes|Yes|YES|true|on) echo true ;;
    n|N|no|No|NO|false|off) echo false ;;
  esac
}
if [ -n "$ALLOW_ELEVATED" ]; then
  v="$(norm_yes_no "$ALLOW_ELEVATED")"
  [ -n "$v" ] || die "--allow-elevated-tokens must be yes or no (got $ALLOW_ELEVATED)."
  ALLOW_ELEVATED="$v"
fi
valid_account() { case "$1" in ""|-*|*[!a-z0-9_-]*) return 1 ;; *) return 0 ;; esac; }
valid_cluster() { case "$1" in ""|-*|*[!A-Za-z0-9._-]*) return 1 ;; *) return 0 ;; esac; }
valid_account "$TERMINAL_ACCOUNT" || die "--terminal-account must use only a-z, 0-9, _ and - (got $TERMINAL_ACCOUNT)."
valid_cluster "$CLUSTER" || die "--cluster must use only A-Z, a-z, 0-9, ., _ and - (got $CLUSTER)."

# ------------------------------------------------------------------ checks
[ "$(id -u)" = 0 ] || die "Run as root: sudo ./install.sh …"

# ------------------------------------------------------------------ uninstall
if [ "$UNINSTALL" = true ]; then
  banner "Orbyn Forge $DOT uninstall" "host $(hostname)"
  if [ "$PURGE" = true ]; then what="the service, $PREFIX, $CONF_DIR, $DATA_DIR (database, keys, CA, backups) and the forge user"
  else what="the service and $PREFIX (configuration and data are kept)"; fi
  if [ "$ASSUME_YES" = false ]; then
    [ -t 0 ] || die "Pass --yes to uninstall without a terminal."
    printf '  Remove %s? [y/N] ' "$what"; ans=""; read -r ans || ans=""
    [ "$(norm_yes_no "$ans")" = true ] || { printf '\n  Nothing was changed.\n\n'; exit 0; }
  fi
  if command -v systemctl >/dev/null 2>&1; then
    systemctl disable --now forge-server >/dev/null 2>&1 || true
    rm -f /etc/systemd/system/forge-server.service
    systemctl daemon-reload >/dev/null 2>&1 || true
  fi
  step "Service" "forge-server.service stopped and removed"
  case "$PREFIX" in /|/usr|/usr/*|/etc|/var|"") die "refusing to remove --prefix $PREFIX" ;; esac
  rm -rf "$PREFIX"
  step "Binaries" "$PREFIX removed"
  if [ "$PURGE" = true ]; then
    rm -rf "$CONF_DIR" "$DATA_DIR"
    step "Configuration, data" "$CONF_DIR and $DATA_DIR removed"
    if id forge >/dev/null 2>&1; then userdel forge >/dev/null 2>&1 || true; fi
    step "Service account" "forge removed"
  else
    step "Kept" "$CONF_DIR and $DATA_DIR (re-install keeps using them; --purge deletes them)"
  fi
  if systemctl cat forge-agent.service >/dev/null 2>&1; then
    note "Agent" "forge-agent on this machine is a node; remove it from the console (Nodes) first"
  fi
  printf '\n  Firewall rules for the Forge ports are left in place.\n\n'
  exit 0
fi
[ "$PURGE" = false ] || die "--purge goes with --uninstall."

[ -d /run/systemd/system ] || die "systemd is required; see the docs for running forge-server under another supervisor."
HERE="$(cd "$(dirname "$0")" && pwd)"
[ -x "$HERE/forge-server" ] || die "Run this from the extracted package (forge-server not found)."
# The package must be whole and for this machine before anything changes.
case "$(uname -m)" in x86_64|amd64) MACH=x86_64 ;; aarch64|arm64) MACH=aarch64 ;; *) MACH="$(uname -m)" ;; esac
case "$(basename "$HERE")" in
  forge-*-x86_64-linux|forge-*-aarch64-linux)
    PKG_ARCH="$(basename "$HERE" | sed -E 's/^forge-.*-(x86_64|aarch64)-linux$/\1/')"
    [ "$PKG_ARCH" = "$MACH" ] || die "This package is for $PKG_ARCH; this machine is $MACH. Download forge-<version>-$MACH-linux.tar.gz." ;;
esac
for f in forge-server forge-plugin-runner forge-analytics-worker dist/SHA256SUMS dist/MANIFEST.json; do
  [ -f "$HERE/$f" ] || die "The package is incomplete: $f is missing. Download it again."
done
if command -v sha256sum >/dev/null 2>&1; then
  (cd "$HERE/dist" && sha256sum --quiet -c SHA256SUMS >/dev/null 2>&1) \
    || die "The agent binaries in $HERE/dist do not match dist/SHA256SUMS. Download the package again."
fi
NEW_VERSION="$("$HERE/forge-server" --version 2>/dev/null | awk '{print $2}')" || true
[ -n "$NEW_VERSION" ] || die "$HERE/forge-server does not run on this machine (wrong architecture or a damaged download)."
"$HERE/forge-analytics-worker" --version >/dev/null 2>&1 \
  || die "$HERE/forge-analytics-worker does not run on this machine (wrong architecture or a damaged download)."
FREE_MB="$(df -Pm "$(dirname "$PREFIX")" 2>/dev/null | awk 'NR == 2 { print $4 }')"
[ -z "$FREE_MB" ] || [ "$FREE_MB" -ge 500 ] || die "Only ${FREE_MB} MB free under $(dirname "$PREFIX"); the installer needs 500 MB (binaries and their *.prev copies for updates)."
OLD_VERSION=""
[ -x "$PREFIX/forge-server" ] && OLD_VERSION="$("$PREFIX/forge-server" --version 2>/dev/null | awk '{print $2}')" || true

HOST="$(hostname -f 2>/dev/null || hostname)"
[ -n "$PUBLIC_URL" ] || PUBLIC_URL="http://$HOST:8080"
if [ -z "$AGENT_HOST" ]; then
  AGENT_HOST="$(printf '%s' "$PUBLIC_URL" | sed -E 's#^[a-z]+://##; s#[:/].*$##')"
fi
SECURE=false
case "$PUBLIC_URL" in https://*) SECURE=true ;; esac

if [ -n "$OLD_VERSION" ]; then MODE="Upgrade $OLD_VERSION → $NEW_VERSION"; else MODE="New installation"; fi
banner "Orbyn Forge $DOT control plane installer" "$MODE  $DOT  version $NEW_VERSION  $DOT  host $HOST"

# ------------------------------------------------------------------ questions
# Only on a terminal, without --yes and without options that choose settings:
# scripts, CI and the lab keep the flag-driven behaviour.
INTERACTIVE=false
if [ -t 0 ] && [ -t 1 ] && [ "$ASSUME_YES" = false ] && [ "$SETTINGS_GIVEN" = false ]; then
  INTERACTIVE=true
fi
NEW_DB=false
# A database is configured when forge.toml names one (old database_url or a
# [database] url); a new SQLite install has no forge.db yet.
db_configured() { [ -f "$CONF_DIR/forge.toml" ] && grep -qE '^(database_url|url[[:space:]]*=[[:space:]]*"postgres)' "$CONF_DIR/forge.toml"; }
if [ ! -f "$DATA_DIR/forge.db" ] && ! db_configured; then
  NEW_DB=true
fi
FRESH_CONF=false
[ -f "$CONF_DIR/forge.toml" ] || FRESH_CONF=true
case "$DB_KIND" in ""|sqlite|postgres) ;; *) die "--db must be sqlite or postgres." ;; esac
[ "$ANALYTICS" = embedded ] && ANALYTICS=chdb   # the older name for chdb
case "$ANALYTICS" in ""|off|chdb|clickhouse|duckdb) ;; *) die "--analytics must be duckdb, chdb, clickhouse or off." ;; esac
case "$DB_URL $ANALYTICS_URL" in *://*:*@*) die "Put database passwords in FORGE_DATABASE_PASSWORD or FORGE_ANALYTICS_PASSWORD, not in the URL." ;; esac
for n in "$DB_NAME" "$ANALYTICS_NAME"; do
  case "$n" in *[!A-Za-z0-9_]*) die "Database names use only letters, digits and _." ;; esac
done

STTY_SAVED=""
restore_tty() { [ -z "$STTY_SAVED" ] || stty "$STTY_SAVED" 2>/dev/null || true; STTY_SAVED=""; }
trap 'restore_tty' EXIT
trap 'restore_tty; printf "\n"; exit 130' INT TERM HUP

QN=0
question() { QN=$((QN + 1)); printf '  %s%s.%s %s %s[%s]%s ' "$C" "$QN" "$N" "$1" "$D" "$2" "$N"; }
ask() { # ask VAR "question" default
  question "$2" "$3"
  ans=""; read -r ans || ans=""
  [ -n "$ans" ] || ans="$3"
  eval "$1=\$ans"
}
ask_yn() { # ask_yn VAR "question" y|n  -> VAR=true|false
  while :; do
    if [ "$3" = y ]; then question "$2" "Y/n"; else question "$2" "y/N"; fi
    ans=""; read -r ans || ans=""
    [ -n "$ans" ] || ans="$3"
    v="$(norm_yes_no "$ans")"
    if [ -n "$v" ]; then eval "$1=\$v"; return 0; fi
    QN=$((QN - 1)); printf '     %sAnswer y or n.%s\n' "$Y" "$N"
  done
}
read_secret() { # read_secret "prompt" -> SECRET (input hidden)
  printf '     %s ' "$1"
  STTY_SAVED="$(stty -g 2>/dev/null || true)"
  stty -echo 2>/dev/null || true
  SECRET=""; read -r SECRET || SECRET=""
  restore_tty
  printf '\n'
}

if [ "$INTERACTIVE" = true ]; then
  printf '  %sAnswer each question, or press Enter for the default in brackets.%s\n\n' "$D" "$N"
  if [ ! -f "$CONF_DIR/forge.toml" ]; then
    IP="$(hostname -I 2>/dev/null | awk '{print $1}')"
    ask PUBLIC_URL "Public URL users open" "http://${IP:-$HOST}:8080"
    AGENT_HOST="$(printf '%s' "$PUBLIC_URL" | sed -E 's#^[a-z]+://##; s#[:/].*$##')"
    case "$PUBLIC_URL" in https://*) SECURE=true ;; *) SECURE=false ;; esac
  fi
  while :; do
    ask ENVIRONMENT "Environment (production, staging, development, test)" "development"
    case "$ENVIRONMENT" in production|staging|development|test) break ;; esac
    QN=$((QN - 1)); printf '     %sChoose production, staging, development or test.%s\n' "$Y" "$N"
  done
  if [ "$FRESH_CONF" = true ]; then
    while :; do
      ask DB_KIND "Operational database (sqlite, postgres)" "sqlite"
      case "$DB_KIND" in sqlite|postgres) break ;; esac
      QN=$((QN - 1)); printf '     %sChoose sqlite or postgres.%s\n' "$Y" "$N"
    done
    if [ "$DB_KIND" = postgres ]; then
      ask_yn DB_LOCAL "Install PostgreSQL on this machine? (n: use an existing server)" y
      if [ "$DB_LOCAL" = false ]; then
        while :; do
          ask DB_URL "PostgreSQL server (no password)" "postgres://forge@db.example.com:5432"
          case "$DB_URL" in
            postgres://*:*@*|postgresql://*:*@*) printf '     %sLeave the password out; it is asked next.%s\n' "$Y" "$N" ;;
            postgres://*|postgresql://*) break ;;
            *) printf '     %sStart with postgres://%s\n' "$Y" "$N" ;;
          esac
          QN=$((QN - 1))
        done
        question "Database password (hidden)" "none"; printf '\n'
        read_secret "Password:"
        if [ -n "$SECRET" ]; then FORGE_DATABASE_PASSWORD="$SECRET"; export FORGE_DATABASE_PASSWORD; SECRET=""; fi
      fi
      while :; do
        ask DB_NAME "Database name (created if missing, reused if it exists)" "forge_op"
        case "$DB_NAME" in
          ""|*[!A-Za-z0-9_]*) QN=$((QN - 1)); printf '     %sUse only letters, digits and _.%s\n' "$Y" "$N" ;;
          *) break ;;
        esac
      done
    fi
    d=duckdb
    while :; do
      ask ANALYTICS "Analytics database for node history (duckdb, chdb, clickhouse, off)" "$d"
      [ "$ANALYTICS" = embedded ] && ANALYTICS=chdb
      case "$ANALYTICS" in off|chdb|clickhouse|duckdb) break ;; esac
      QN=$((QN - 1)); printf '     %sChoose duckdb, chdb, clickhouse or off.%s\n' "$Y" "$N"
    done
    if [ "$ANALYTICS" = clickhouse ]; then
      ask ANALYTICS_URL "ClickHouse server (no password)" "https://default@clickhouse.example.com:8443"
      question "Analytics password (hidden)" "none"; printf '\n'
      read_secret "Password:"
      if [ -n "$SECRET" ]; then FORGE_ANALYTICS_PASSWORD="$SECRET"; export FORGE_ANALYTICS_PASSWORD; SECRET=""; fi
    fi
  fi
  if [ "$NEW_DB" = true ]; then
    ask ADMIN_USER "Administrator user name" "admin"
    while :; do
      question "Administrator password (hidden; Enter generates one)" "generate"
      printf '\n'
      read_secret "Password:"
      if [ -z "$SECRET" ]; then unset FORGE_ADMIN_PASSWORD; break; fi
      if [ "${#SECRET}" -lt 12 ]; then
        SECRET=""; QN=$((QN - 1)); printf '     %sUse at least 12 characters.%s\n' "$Y" "$N"; continue
      fi
      p1="$SECRET"; read_secret "Repeat:  "
      if [ "$p1" = "$SECRET" ]; then FORGE_ADMIN_PASSWORD="$p1"; export FORGE_ADMIN_PASSWORD; p1=""; SECRET=""; break; fi
      p1=""; SECRET=""; QN=$((QN - 1)); printf '     %sThe passwords differ. Try again.%s\n' "$Y" "$N"
    done
  fi
  # MFA is each user's choice for their own sign-in; no question. Elevated
  # tokens only matter when --mfa required makes guarded actions need MFA.
  if [ "$MFA" = required ]; then
    if [ "$ENVIRONMENT" = production ]; then d=n; else d=y; fi
    ask_yn ALLOW_ELEVATED "Allow elevated API tokens for CI (no MFA prompt)?" "$d"
  fi
  ask_yn INSTALL_AGENT "Also run workloads on this server (install the Forge agent here)?" n
  if [ "$INSTALL_AGENT" = true ]; then
    while :; do
      ask CLUSTER "Cluster for this server (created if missing)" "default"
      if valid_cluster "$CLUSTER"; then break; fi
      QN=$((QN - 1)); printf '     %sUse only A-Z, a-z, 0-9, ., _ and -.%s\n' "$Y" "$N"
    done
    while :; do
      ask TERMINAL_ACCOUNT "Terminal account on this server" "forge-ops"
      if valid_account "$TERMINAL_ACCOUNT"; then break; fi
      QN=$((QN - 1)); printf '     %sUse only a-z, 0-9, _ and -.%s\n' "$Y" "$N"
    done
  fi

  section "REVIEW"
  [ -f "$CONF_DIR/forge.toml" ] || row "Public URL" "$PUBLIC_URL"
  row "Environment" "$ENVIRONMENT"
  if [ "$FRESH_CONF" = true ]; then
    if [ "$DB_KIND" != postgres ]; then row "Database" "SQLite in $DATA_DIR"
    elif [ "$DB_LOCAL" = true ]; then row "Database" "PostgreSQL on this machine, ${DB_NAME:-forge_op}"
    else row "Database" "$DB_URL/${DB_NAME:-forge_op}"; fi
    row "Analytics" "${ANALYTICS:-off}"
  fi
  if [ "$NEW_DB" = true ]; then
    row "Admin" "$ADMIN_USER"
    if [ -n "${FORGE_ADMIN_PASSWORD:-}" ]; then row "Password" "as typed"; else row "Password" "generated, shown once at the end"; fi
  fi
  if [ "$MFA" = required ]; then row "MFA" "required for guarded actions"; else row "MFA" "each user's choice (sign-in only)"; fi
  if [ "$MFA" = required ]; then
    if [ "$ALLOW_ELEVATED" = true ]; then row "Elevated" "API tokens allowed"; else row "Elevated" "API tokens not allowed"; fi
  fi
  if [ "$INSTALL_AGENT" = true ]; then row "Agent" "yes: cluster $CLUSTER, terminal account $TERMINAL_ACCOUNT"; else row "Agent" "no"; fi
  printf '\n'
  ask_yn GO "Install with these settings?" y
  [ "$GO" = true ] || { printf '\n  Nothing was changed.\n\n'; exit 0; }
  printf '\n'
fi

# Elevated API tokens on a new forge.toml: off only for hardened production.
if [ -z "$ALLOW_ELEVATED" ] && [ ! -f "$CONF_DIR/forge.toml" ]; then
  if [ "${ENVIRONMENT:-production}" = production ] && [ "$MFA" = required ]; then
    ALLOW_ELEVATED=false
  else
    ALLOW_ELEVATED=true
  fi
fi

# ------------------------------------------------------------------ install
id forge >/dev/null 2>&1 || useradd --system --home-dir "$DATA_DIR" --shell /usr/sbin/nologin forge
step "Service account" "forge (system user, no login)"
install -d -m 0755 "$PREFIX" "$PREFIX/dist" "$CONF_DIR"
install -d -m 0700 -o forge -g forge "$DATA_DIR"
# Existing installs: the database, keys, CA, backups and staged updates are
# the service user's only (the server checks this again at every start).
# (initial-admin-password stays root-only.)
find "$DATA_DIR" -xdev ! -name initial-admin-password -exec chown forge:forge {} +
chmod 0700 "$DATA_DIR"
find "$DATA_DIR" -xdev \( -type d -exec chmod 0700 {} + \) -o \( -type f -exec chmod go-rwx {} + \)
step "Directories" "$PREFIX $DOT $CONF_DIR $DOT $DATA_DIR"

# Install by copy + rename: a running server keeps its file until restart.
for f in forge-server forge-plugin-runner forge-analytics-worker; do
  install -m 0755 "$HERE/$f" "$PREFIX/$f.new" && mv -f "$PREFIX/$f.new" "$PREFIX/$f"
done
cp -f "$HERE"/dist/* "$PREFIX/dist/"
chmod 0755 "$PREFIX"/dist/forge-agent* 2>/dev/null || true
# The self-updater replaces these files, as the service user.
chown -R forge:forge "$PREFIX"
# SELinux (RHEL-likes): default labels for the binaries, also under a custom --prefix.
if command -v restorecon >/dev/null 2>&1; then
  restorecon -RF "$PREFIX" "$CONF_DIR" "$DATA_DIR" 2>/dev/null || true
fi
# A copy of this script for --uninstall later, root's only: not in $PREFIX,
# which the service user owns (the self-updater writes there).
install -m 0700 -o root -g root "$HERE/install.sh" "$CONF_DIR/install.sh.new" && mv -f "$CONF_DIR/install.sh.new" "$CONF_DIR/install.sh"
AGENTS=$(find "$PREFIX/dist" -maxdepth 1 -name 'forge-agent*' ! -name '*.prev' | wc -l | tr -d ' ')
step "Binaries" "server, analytics worker, plugin runner, $AGENTS agent binaries"

if [ ! -f "$CONF_DIR/forge.toml" ]; then
  cat > "$CONF_DIR/forge.toml" <<EOF
# Orbyn Forge control plane (written by install.sh; see forge.toml.example).
data_dir = "$DATA_DIR"
api_listen = "0.0.0.0:8080"
public_url = "$PUBLIC_URL"
cookie_secure = $SECURE
agent_listen = "0.0.0.0:7443"
agent_public_url = "https://$AGENT_HOST:7443"
agent_hostnames = ["$AGENT_HOST", "$HOST", "localhost", "127.0.0.1"]
agent_dist_dir = "$PREFIX/dist"

[update]
enabled = true

[security]
# MFA is each user's choice and protects their sign-in. true also requires it
# for guarded actions. Change with: forge-server --config $CONF_DIR/forge.toml mfa on|off
require_mfa = false
# Elevated API tokens (for CI) skip the MFA prompt when true.
allow_elevated_tokens = ${ALLOW_ELEVATED:-false}
# Reverse proxies (ingress edges) whose X-Forwarded-For/-Proto are trusted.
trusted_proxies = [$(printf '%s' "$TRUSTED_PROXIES" | tr -d ' "' | sed -E 's/[^,]+/"&"/g')]
EOF
  chmod 0640 "$CONF_DIR/forge.toml"
  chgrp forge "$CONF_DIR/forge.toml"
  step "Configuration" "$CONF_DIR/forge.toml (new)"
else
  step "Configuration" "$CONF_DIR/forge.toml (kept)"
  # Existing file: change allow_elevated_tokens in place, only when chosen.
  if [ -n "$ALLOW_ELEVATED" ]; then
    f="$CONF_DIR/forge.toml"
    if grep -q '^allow_elevated_tokens *=' "$f"; then
      sed -i "s/^allow_elevated_tokens *=.*/allow_elevated_tokens = $ALLOW_ELEVATED/" "$f"
    elif grep -q '^\[security\]' "$f"; then
      sed -i "/^\[security\]/a allow_elevated_tokens = $ALLOW_ELEVATED" "$f"
    else
      printf '\n[security]\nallow_elevated_tokens = %s\n' "$ALLOW_ELEVATED" >> "$f"
    fi
  fi
fi
if grep -q '^allow_elevated_tokens *= *true' "$CONF_DIR/forge.toml"; then
  ELEVATED_STATE="allowed"
  step "Elevated tokens" "allowed: elevated API tokens skip the MFA prompt"
else
  ELEVATED_STATE="not allowed"
  step "Elevated tokens" "not allowed"
fi

# Top-level keys go before the first [table], so they are inserted after line 1.
set_top_key() {
  sed -i "/^$1 = /d" "$CONF_DIR/forge.toml"
  sed -i "1a $1 = $2" "$CONF_DIR/forge.toml"
}
if [ -n "$ENVIRONMENT" ]; then
  set_top_key environment "\"$ENVIRONMENT\""
  step "Environment" "$ENVIRONMENT"
fi
# MFA: the same editor as `forge-server mfa` (keeps comments, owner and mode).
if [ -n "$MFA" ]; then
  if [ "$MFA" = off ]; then MFA_CMD=off; else MFA_CMD=on; fi
  "$PREFIX/forge-server" --config "$CONF_DIR/forge.toml" mfa "$MFA_CMD" >/dev/null \
    || die "Could not set MFA in $CONF_DIR/forge.toml."
fi
if "$PREFIX/forge-server" --config "$CONF_DIR/forge.toml" mfa status 2>/dev/null | grep -q 'not required'; then
  step "MFA" "each user's choice (protects sign-in; forge-server mfa on also guards actions)"
  MFA_STATE="each user's choice"
else
  step "MFA" "required for guarded actions"
  MFA_STATE="required"
fi

# ------------------------------------------------------------------ databases
write_secret() { # write_secret FILE VALUE: readable by the forge user only
  old_umask=$(umask); umask 077; printf '%s\n' "$2" > "$1"; umask "$old_umask"
  chown forge:forge "$1"; chmod 0600 "$1"
}
if [ "$FRESH_CONF" = true ]; then
  if [ -z "$ANALYTICS" ]; then
    ANALYTICS=duckdb
  fi
  if [ "$DB_LOCAL" = true ]; then
    # The server, not just the client: a host may have psql without a server.
    if ! systemctl cat postgresql.service >/dev/null 2>&1; then
      if command -v apt-get >/dev/null 2>&1; then
        DEBIAN_FRONTEND=noninteractive apt-get install -y -q postgresql >/dev/null
      elif command -v dnf >/dev/null 2>&1; then
        # EL8's default stream is PostgreSQL 10; take a supported one.
        case "$(. /etc/os-release; echo "${VERSION_ID%%.*}")" in
          8) dnf module reset -y -q postgresql >/dev/null 2>&1 || true
             dnf module enable -y -q postgresql:15 >/dev/null ;;
        esac
        dnf install -y -q postgresql-server >/dev/null
      else
        die "Install PostgreSQL yourself, then re-run with --db-url."
      fi
    fi
    # RHEL-likes ship the cluster uninitialised, and their pg_hba.conf uses
    # `ident` for local TCP, which refuses the password login Forge uses.
    if [ -d /var/lib/pgsql ] && command -v postgresql-setup >/dev/null 2>&1; then
      pgdata=/var/lib/pgsql/data
      if [ ! -s "$pgdata/PG_VERSION" ]; then
        postgresql-setup --initdb >/dev/null || die "postgresql-setup --initdb failed (see journalctl -u postgresql)"
      fi
      sed -i -E 's/^(host[[:space:]]+all[[:space:]]+all[[:space:]]+(127\.0\.0\.1\/32|::1\/128)[[:space:]]+)ident[[:space:]]*$/\1scram-sha-256/' "$pgdata/pg_hba.conf"
      grep -qE "^password_encryption[[:space:]]*=[[:space:]]*'?scram-sha-256" "$pgdata/postgresql.conf" \
        || printf "\npassword_encryption = 'scram-sha-256'  # Forge installer\n" >> "$pgdata/postgresql.conf"
      systemctl enable postgresql >/dev/null 2>&1 || true
      systemctl restart postgresql || die "PostgreSQL did not start (see journalctl -u postgresql)"
    fi
    systemctl enable --now postgresql >/dev/null 2>&1 || true
    # A login role `forge` with a generated password that may create databases.
    if [ -z "${FORGE_DATABASE_PASSWORD:-}" ]; then
      FORGE_DATABASE_PASSWORD="$(LC_ALL=C tr -dc 'A-Za-z0-9' </dev/urandom | head -c 32)"
      export FORGE_DATABASE_PASSWORD
    fi
    if runuser -u postgres -- psql -tAc "SELECT 1 FROM pg_roles WHERE rolname='forge'" | grep -q 1; then
      verb=ALTER
    else
      verb=CREATE
    fi
    # The password is generated from [A-Za-z0-9] or typed by the operator;
    # psql reads it from stdin so it never shows in the process list.
    printf "%s ROLE forge WITH LOGIN CREATEDB PASSWORD '%s';\n" "$verb" "$(printf '%s' "$FORGE_DATABASE_PASSWORD" | sed "s/'/''/g")" \
      | runuser -u postgres -- psql -q >/dev/null
    DB_URL="postgres://forge@127.0.0.1:5432"
    step "PostgreSQL" "on this machine, role forge"
  fi
  {
    printf '\n[database]\n'
    if [ "$DB_KIND" = postgres ]; then
      printf 'url = "%s"\nname = "%s"\n' "$DB_URL" "${DB_NAME:-forge_op}"
      if [ -n "${FORGE_DATABASE_PASSWORD:-}" ]; then
        write_secret "$CONF_DIR/db-password" "$FORGE_DATABASE_PASSWORD"
        printf 'password_file = "%s"\n' "$CONF_DIR/db-password"
      fi
    fi
    printf '\n[analytics]\nengine = "%s"\n' "$ANALYTICS"
    [ -z "$ANALYTICS_URL" ] || printf 'url = "%s"\n' "$ANALYTICS_URL"
    [ -z "$ANALYTICS_NAME" ] || printf 'name = "%s"\n' "$ANALYTICS_NAME"
    if [ -n "${FORGE_ANALYTICS_PASSWORD:-}" ]; then
      write_secret "$CONF_DIR/analytics-password" "$FORGE_ANALYTICS_PASSWORD"
      printf 'password_file = "%s"\n' "$CONF_DIR/analytics-password"
    fi
  } >> "$CONF_DIR/forge.toml"
  unset FORGE_DATABASE_PASSWORD FORGE_ANALYTICS_PASSWORD
  if [ "$DB_KIND" = postgres ]; then
    set -- db check --create
    [ "$ALLOW_SHARED_DB" = false ] || set -- "$@" --allow-shared
    if ! out=$(runuser -u forge -- "$PREFIX/forge-server" --config "$CONF_DIR/forge.toml" "$@" 2>&1); then
      printf '%s\n' "$out" >&2
      die "The database cannot be used (see above). Fix it and re-run the installer."
    fi
    step "Database" "$(printf '%s' "$out" | sed -n 's/^contents: //p')"
    # A reused Forge database already has its administrator.
    case "$out" in *"contents: Forge database"*) NEW_DB=false ;; *) NEW_DB=true ;; esac
  fi
fi

# ------------------------------------------------------------------ administrator
ADMIN_CREATED=false
PASSWORD_FILE="$DATA_DIR/initial-admin-password"
GENERATED=false
if [ "$NEW_DB" = true ]; then
  if [ -n "${FORGE_ADMIN_PASSWORD:-}" ]; then
    [ "${#FORGE_ADMIN_PASSWORD}" -ge 12 ] || die "FORGE_ADMIN_PASSWORD must be at least 12 characters."
  else
    # 24 characters from an unambiguous alphabet (no 0/O, 1/l/I): about 137 bits.
    FORGE_ADMIN_PASSWORD="$(LC_ALL=C tr -dc 'ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789' </dev/urandom | head -c 24)"
    [ "${#FORGE_ADMIN_PASSWORD}" -eq 24 ] || die "Could not generate a password."
    GENERATED=true
    old_umask=$(umask); umask 077
    {
      echo "# Orbyn Forge initial administrator. Forge deletes this file when the"
      echo "# administrator changes the password (console: Profile > Password & security)."
      echo "username: $ADMIN_USER"
      echo "password: $FORGE_ADMIN_PASSWORD"
    } > "$PASSWORD_FILE"
    umask "$old_umask"
    chown root:root "$PASSWORD_FILE"
    chmod 0600 "$PASSWORD_FILE"
  fi
  set -- init --username "$ADMIN_USER"
  [ -z "$ADMIN_EMAIL" ] || set -- "$@" --email "$ADMIN_EMAIL"
  [ "$GENERATED" = false ] || set -- "$@" --generated-password
  # `init` validates everything before it creates the database. If it still
  # fails, remove what this run created so a re-run starts clean.
  if ! out=$(FORGE_ADMIN_PASSWORD="$FORGE_ADMIN_PASSWORD" runuser -u forge -- \
    "$PREFIX/forge-server" --config "$CONF_DIR/forge.toml" "$@" 2>&1); then
    rm -f "$DATA_DIR/forge.db" "$DATA_DIR/forge.db-wal" "$DATA_DIR/forge.db-shm" "$PASSWORD_FILE"
    printf '%s\n' "$out" | sed 's/^/      /' >&2
    die "Could not create the administrator. Nothing was kept; fix the problem and run the installer again."
  fi
  ADMIN_CREATED=true
  step "Administrator" "$ADMIN_USER${ADMIN_EMAIL:+ <$ADMIN_EMAIL>} (platform admin)"
else
  step "Administrator" "existing accounts kept"
fi

if [ -n "$PUBLIC_ADDRESS" ]; then
  if ! out=$(runuser -u forge -- "$PREFIX/forge-server" --config "$CONF_DIR/forge.toml" \
    public-address set "$PUBLIC_ADDRESS" 2>&1); then
    printf '%s\n' "$out" | sed 's/^/      /' >&2
    die "The public address was refused; fix it and run the installer again."
  fi
  step "Public address" "$PUBLIC_ADDRESS"
fi

# ------------------------------------------------------------------ service
cat > /etc/systemd/system/forge-server.service <<EOF
[Unit]
Description=Orbyn Forge control plane
After=network-online.target
Wants=network-online.target

[Service]
User=forge
Group=forge
ExecStart=$PREFIX/forge-server --config $CONF_DIR/forge.toml serve
Restart=always
RestartSec=3
LimitNOFILE=65536
# Two malloc arenas: glibc otherwise keeps a free-memory arena per thread
# (D-142 heap profile: 1,000 nodes, 142 MB RSS -> 114 MB, flat).
Environment=MALLOC_ARENA_MAX=2
UMask=0077
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=full
ProtectHome=true
ReadWritePaths=$DATA_DIR $PREFIX

[Install]
WantedBy=multi-user.target
EOF
command -v restorecon >/dev/null 2>&1 && restorecon -F /etc/systemd/system/forge-server.service 2>/dev/null || true
systemctl daemon-reload
systemctl enable forge-server >/dev/null 2>&1
systemctl restart forge-server
step "Service" "forge-server.service (enabled, restarts on failure; starts the analytics worker)"

# ------------------------------------------------------------------ ports
# From the configuration (a re-run keeps the operator's ports).
conf_value() { # conf_value <table or -> <key>: a quoted value from forge.toml
  awk -v t="$1" -v k="$2" '
    /^[[:space:]]*\[/ { cur = $0; gsub(/[][[:space:]]/, "", cur); next }
    (t == "-" && cur == "") || cur == t {
      if (match($0, "^[[:space:]]*" k "[[:space:]]*=[[:space:]]*\"")) {
        v = substr($0, RLENGTH + 1); sub(/".*/, "", v); print v; exit
      }
    }' "$CONF_DIR/forge.toml"
}
port_of() { printf '%s' "$1" | sed -n 's/.*:\([0-9][0-9]*\)$/\1/p'; }
API_LISTEN="$(conf_value - api_listen)"; API_LISTEN="${API_LISTEN:-0.0.0.0:8080}"
API_PORT="$(port_of "$API_LISTEN")"; API_PORT="${API_PORT:-8080}"
AGENT_PORT="$(port_of "$(conf_value - agent_listen)")"; AGENT_PORT="${AGENT_PORT:-7443}"
ANALYTICS_ENGINE="$(conf_value analytics engine)"; ANALYTICS_ENGINE="${ANALYTICS_ENGINE:-off}"
WORKER_PORT="$(port_of "$(conf_value analytics worker_agent_listen)")"; WORKER_PORT="${WORKER_PORT:-55132}"
# The local address for checks: the API's own host unless it listens on all.
API_HOST="$(printf '%s' "$API_LISTEN" | sed -E 's/:[0-9]+$//; s/^\[(.*)\]$/\1/')"
case "$API_HOST" in ""|0.0.0.0|::) LOCAL_API="http://127.0.0.1:$API_PORT" ;; *:*) LOCAL_API="http://[$API_HOST]:$API_PORT" ;; *) LOCAL_API="http://$API_HOST:$API_PORT" ;; esac
PORTS="$API_PORT $AGENT_PORT $WORKER_PORT"
PORTS_TEXT="$API_PORT/tcp (console), $AGENT_PORT/tcp (agents), $WORKER_PORT/tcp (analytics uploads)"

# Plain iptables (for example Oracle Cloud images: SSH allowed, then a final
# REJECT): insert an ACCEPT per port above the first REJECT or DROP rule, for
# IPv4 and IPv6, and save the rules so they survive a reboot. Returns 1 when
# iptables is missing or blocks nothing (nothing to open). Ports that are
# already accepted are left alone, so a re-run adds no duplicates.
IPT_SAVED="no"
open_iptables_ports() {
  command -v iptables >/dev/null 2>&1 || return 1
  found=false
  for ipt in iptables ip6tables; do
    command -v "$ipt" >/dev/null 2>&1 || continue
    "$ipt" -S INPUT >/dev/null 2>&1 || continue
    pos="$("$ipt" -L INPUT -n --line-numbers 2>/dev/null | awk '$2=="REJECT"||$2=="DROP"{print $1; exit}')"
    policy_drop=false
    "$ipt" -S INPUT 2>/dev/null | grep -q '^-P INPUT DROP' && policy_drop=true
    [ -n "$pos" ] || [ "$policy_drop" = true ] || continue
    found=true
    for p in $PORTS; do
      "$ipt" -C INPUT -p tcp -m state --state NEW --dport "$p" -j ACCEPT 2>/dev/null && continue
      # A rule that cannot be added must not fail the install: the server is
      # already set up, and the summary tells the user to open the port.
      if [ -n "$pos" ]; then
        "$ipt" -I INPUT "$pos" -p tcp -m state --state NEW --dport "$p" -j ACCEPT || return 1
      else
        "$ipt" -A INPUT -p tcp -m state --state NEW --dport "$p" -j ACCEPT || return 1
      fi
    done
  done
  [ "$found" = true ] || return 1
  if command -v netfilter-persistent >/dev/null 2>&1; then
    netfilter-persistent save >/dev/null 2>&1 && IPT_SAVED="netfilter-persistent"
  elif [ -d /etc/iptables ]; then
    iptables-save > /etc/iptables/rules.v4 2>/dev/null && IPT_SAVED="/etc/iptables/rules.v4"
    command -v ip6tables-save >/dev/null 2>&1 && ip6tables-save > /etc/iptables/rules.v6 2>/dev/null
  elif [ -f /etc/sysconfig/iptables ] && command -v iptables-save >/dev/null 2>&1; then
    iptables-save > /etc/sysconfig/iptables 2>/dev/null && IPT_SAVED="/etc/sysconfig/iptables"
  fi
  return 0
}

if command -v firewall-cmd >/dev/null 2>&1 && firewall-cmd --state >/dev/null 2>&1; then
  for p in $PORTS; do firewall-cmd -q --permanent --add-port="$p/tcp"; done
  firewall-cmd -q --reload
  step "Firewall" "opened $PORTS_TEXT (firewalld)"
elif command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -q '^Status: active'; then
  for p in $PORTS; do ufw allow "$p/tcp" >/dev/null; done
  step "Firewall" "opened $PORTS_TEXT (ufw)"
elif open_iptables_ports; then
  step "Firewall" "opened $PORTS_TEXT (iptables, saved: $IPT_SAVED)"
else
  step "Firewall" "no active firewalld, ufw or blocking iptables rules; allow $PORTS_TEXT in yours"
fi
note "Cloud firewall" "also allow TCP $API_PORT, $AGENT_PORT and $WORKER_PORT in your provider's firewall (security group, security list); the installer cannot change it"

i=0
until curl -fsS -o /dev/null "$LOCAL_API/api/health" 2>/dev/null; do
  i=$((i + 1)); [ "$i" -lt 60 ] || die "forge-server did not start. See: journalctl -u forge-server -n 50"
  sleep 1
done
step "Health check" "responding after ${i}s"

# The analytics worker is forge-server's child; it opens the upload port.
WORKER_STATE="off ([analytics] engine = \"off\")"
if [ "$ANALYTICS_ENGINE" != off ]; then
  # Listening = anything but "refused" or a timeout (the port wants a client
  # certificate, so the TLS handshake itself fails).
  port_open() { rc=0; curl -sk -o /dev/null --max-time 2 "https://127.0.0.1:$1/" 2>/dev/null || rc=$?; [ "$rc" != 7 ] && [ "$rc" != 28 ]; }
  i=0
  until port_open "$WORKER_PORT"; do
    i=$((i + 1))
    if [ "$i" -ge 30 ]; then WORKER_STATE="not listening on $WORKER_PORT yet"; break; fi
    sleep 1
  done
  if [ "$i" -lt 30 ]; then
    WORKER_STATE="$ANALYTICS_ENGINE, uploads on port $WORKER_PORT"
    step "Analytics worker" "$WORKER_STATE"
  else
    note "Analytics worker" "$WORKER_STATE; see journalctl -u forge-server -n 50 (the console works without it)"
  fi
fi

# ------------------------------------------------------------------ agent
# Enroll this server as a node: a short-lived token from the local server
# (passed in the environment, never on a command line or in a log), then the
# server's own node installer. A failure here does not fail the install.
AGENT_STATE="not installed on this server"
AGENT_FAILED=false
if [ "$INSTALL_AGENT" = true ] && systemctl is-active --quiet forge-agent 2>/dev/null; then
  # Re-run: the agent is already enrolled here; a second enrollment would add a duplicate node.
  AGENT_STATE="already enrolled (forge-agent running), skipped"
  INSTALL_AGENT=false
fi
if [ "$INSTALL_AGENT" = true ]; then
  NODE_NAME="$(hostname)"
  AGENT_LOG="$(mktemp)"
  if FORGE_ENROLL_TOKEN="$(runuser -u forge -- "$PREFIX/forge-server" --config "$CONF_DIR/forge.toml" \
        enroll-token --cluster "$CLUSTER" --hostname "$NODE_NAME" --ttl-minutes 15 2>>"$AGENT_LOG")" \
     && [ -n "$FORGE_ENROLL_TOKEN" ]; then
    export FORGE_ENROLL_TOKEN
    if curl -fsSL "$LOCAL_API/install.sh?terminal_account=$TERMINAL_ACCOUNT" 2>>"$AGENT_LOG" \
         | sh >>"$AGENT_LOG" 2>&1; then
      AGENT_STATE="enrolled in cluster $CLUSTER as $NODE_NAME (terminal account $TERMINAL_ACCOUNT)"
      step "Agent" "enrolled in cluster $CLUSTER"
    else
      AGENT_FAILED=true; AGENT_STATE="not enrolled: the node installer failed"
    fi
  else
    AGENT_FAILED=true; AGENT_STATE="not enrolled: could not create an enrollment token"
  fi
  unset FORGE_ENROLL_TOKEN
  if [ "$AGENT_FAILED" = true ]; then
    note "Agent" "$AGENT_STATE; the server install is fine"
    tail -n 15 "$AGENT_LOG" | sed 's/^/      /' >&2
  fi
  rm -f "$AGENT_LOG"
fi

# ------------------------------------------------------------------ summary
# Report what the configuration says (a re-run keeps the existing file).
PUBLIC_URL="$(sed -n 's/^public_url = "\(.*\)"/\1/p' "$CONF_DIR/forge.toml" | head -1)"
AGENT_URL="$(sed -n 's/^agent_public_url = "\(.*\)"/\1/p' "$CONF_DIR/forge.toml" | head -1)"
case "$PUBLIC_URL" in https://*) SECURE=true ;; *) SECURE=false ;; esac

if [ -n "$OLD_VERSION" ]; then
  printf '\n  %s%sOrbyn Forge upgraded to %s and running.%s\n' "$G" "$B" "$NEW_VERSION" "$N"
else
  printf '\n  %s%sOrbyn Forge %s is installed and running.%s\n' "$G" "$B" "$NEW_VERSION" "$N"
fi

section "ACCESS"
row "Console" "$PUBLIC_URL"
row "Agents" "$AGENT_URL  ${D}(nodes connect out to TCP $AGENT_PORT and $WORKER_PORT)${N}"
row "Analytics" "$WORKER_STATE"

if [ "$ADMIN_CREATED" = true ]; then
  section "ADMINISTRATOR"
  row "Username" "$ADMIN_USER"
  [ -z "$ADMIN_EMAIL" ] || row "Email" "$ADMIN_EMAIL"
  if [ "$GENERATED" = true ]; then
    row "Password" "$B$FORGE_ADMIN_PASSWORD$N"
    row "Saved to" "$PASSWORD_FILE ${D}(root only, deleted when you change it)${N}"
  else
    row "Password" "the value of FORGE_ADMIN_PASSWORD"
  fi
  printf '    %s%s Sign in, then in Settings add your email, change the password and set up MFA.%s\n' "$D" "$DOT" "$N"
fi

section "SECURITY"
row "MFA" "$MFA_STATE ${D}(forge-server mfa on|off)${N}"
row "Elevated" "API tokens $ELEVATED_STATE ${D}([security] allow_elevated_tokens)${N}"

section "AGENT"
row "This server" "$AGENT_STATE"
if [ "$AGENT_FAILED" = true ]; then
  printf '    %s%s Retry by hand, in a root shell (sudo -s):%s\n' "$D" "$DOT" "$N"
  printf '      FORGE_ENROLL_TOKEN=$(runuser -u forge -- %s/forge-server --config %s/forge.toml enroll-token --cluster %s --hostname "$(hostname)" --ttl-minutes 15)\n' "$PREFIX" "$CONF_DIR" "$CLUSTER"
  printf "      export FORGE_ENROLL_TOKEN; curl -fsSL '%s/install.sh?terminal_account=%s' | sh; unset FORGE_ENROLL_TOKEN\n" "$LOCAL_API" "$TERMINAL_ACCOUNT"
fi

section "INSTALLED"
row "Binaries" "$PREFIX"
row "Agents" "$PREFIX/dist"
row "Config" "$CONF_DIR/forge.toml"
row "Data" "$DATA_DIR ${D}(database, keys, internal CA, backups)${N}"
row "Service" "forge-server.service ${D}(systemd, user forge)${N}"

section "OPERATE"
row "Status" "systemctl status forge-server"
row "Logs" "journalctl -u forge-server -f"
row "Uninstall" "sudo sh $CONF_DIR/install.sh --uninstall"

section "NEXT STEPS"
printf '    1. Open %s and sign in%s.\n' "$PUBLIC_URL" "$( [ "$ADMIN_CREATED" = true ] && printf ' as %s' "$ADMIN_USER")"
if [ "$GENERATED" = true ]; then
  printf '       The password is also in %s (sudo cat it).\n' "$PASSWORD_FILE"
elif [ "$ADMIN_CREATED" = false ] && [ -f "$PASSWORD_FILE" ]; then
  printf '       First password: sudo cat %s\n' "$PASSWORD_FILE"
fi
printf '    2. Allow TCP %s and %s from your nodes to this server.\n' "$AGENT_PORT" "$WORKER_PORT"
printf '    3. Add nodes: Overview > Add node (docs: /docs/guides/add-nodes/).\n'
if [ "$SECURE" != true ]; then
  printf '\n'
  note "Production" "Put a TLS proxy (Caddy, Nginx) in front and use an https:// public URL."
fi
printf '\n'
unset FORGE_ADMIN_PASSWORD
