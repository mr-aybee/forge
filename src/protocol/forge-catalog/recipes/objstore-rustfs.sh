# RustFS object store instance (forge_catalog::storage). Runs after the
# prelude, as root, with a cleared environment holding only FORGE_* values.
# Native: the release zip (SHA256SUMS verified) as a systemd service.
# Docker: the rustfs/rustfs image. Never deletes the data path.

SLUG="$FORGE_OS_SLUG"
REL="$FORGE_OS_VERSION"
RUNTIME="$FORGE_OS_RUNTIME"
S3_PORT="$FORGE_OS_S3_PORT"
CONSOLE_PORT="$FORGE_OS_CONSOLE_PORT"
DATA="$FORGE_OS_DATA"
DATA_ALL="${FORGE_OS_DATA_ALL:-$DATA}"
# Distributed (HA): every drive of every member, identical on all members.
VOLUMES="${FORGE_OS_VOLUMES:-}"
UNIT="forge-objstore-$SLUG"
# An adopted App Store container keeps its name, data and credential.
ADOPTED="${FORGE_OS_ADOPTED:-0}"
[ "$ADOPTED" != 1 ] || UNIT="forge-rustfs"
# Docker: the container name from the store's settings (D-170).
[ -z "${FORGE_OS_CONTAINER:-}" ] || UNIT="$FORGE_OS_CONTAINER"
# Paths as the engine sees them (named volumes are mounted inside only).
CONTAINER_DATA="${FORGE_OS_CONTAINER_DATA:-$DATA_ALL}"
ENV_ALLOWLIST="${FORGE_OS_ENV_ALLOWLIST:-}"
LEGACY_ENV="/etc/forge/pkgs/rustfs.env"
BASE="/opt/forge/objstore/$SLUG"
ENVF="/etc/forge/objstore/$SLUG.env"
SVC_USER="forge-objstore"
# The image runs as rustfs (10001); native services use the same ids so a
# data path can move between runtimes.
SVC_UID=10001
RELEASES="https://github.com/rustfs/rustfs/releases"

# ---------------------------------------------------------------- helpers
is_mount() {
  if command -v mountpoint >/dev/null 2>&1; then mountpoint -q "$1"; else
    [ "$(stat -c %d "$1" 2>/dev/null)" != "$(stat -c %d "$1/.." 2>/dev/null)" ]
  fi
}

# Refuses to run on a missing mount: the engine must never fill the root
# filesystem because a volume did not come up.
check_data() {
  [ "$FORGE_OS_REQUIRE_MOUNT" = 1 ] || return 0
  for _d in $DATA_ALL; do
    [ -d "$_d" ] || die "data path $_d does not exist; mount the volume first"
    is_mount "$_d" || die "data path $_d is not a mounted volume; mount it (or turn off 'require a dedicated mount')"
  done
}

# ------------------------------------------------------------ ownership
# Every data path carries `.forge-objstore` with the store id. Data is deleted
# only where that marker matches, so a delete never reaches a directory the
# store did not create (a home directory, another store, an app's data).
MARKER=".forge-objstore"
OWNER="${FORGE_OS_OWNER:-}"

write_markers() {
  [ -n "$OWNER" ] || return 0
  for _d in $DATA_ALL; do
    [ -d "$_d" ] && [ ! -L "$_d" ] || continue
    if [ -f "$_d/$MARKER" ]; then
      _cur="$(head -n 1 "$_d/$MARKER" 2>/dev/null | head -c 128)"
      [ "$_cur" = "$OWNER" ] || log "warning: $_d is marked as data of store $_cur; marker kept"
    elif [ -z "$(ls -A "$_d" 2>/dev/null)" ] || [ -d "$_d/.rustfs.sys" ]; then
      # Only an empty directory or one holding RustFS data is claimed: a
      # directory with other files never becomes deletable.
      echo "$OWNER" > "$_d/$MARKER" && chmod 0644 "$_d/$MARKER"
    else
      log "warning: $_d holds other files; it is not marked, and deleting data skips it"
    fi
  done
}

# Whether path $1 equals, contains or lies inside path $2.
paths_overlap() {
  _a="${1%/}"; _b="${2%/}"
  [ "$_a" = "$_b" ] && return 0
  case "$_a/" in "$_b/"*) return 0 ;; esac
  case "$_b/" in "$_a/"*) return 0 ;; esac
  return 1
}

# Deletes the store's data, path by path, skipping anything it cannot prove
# it owns. Skips are logged; the uninstall itself still succeeds.
delete_data_paths() {
  for _d in $DATA_ALL; do
    [ -e "$_d" ] || [ -L "$_d" ] || continue
    if [ -L "$_d" ]; then log "warning: skipped $_d: it is a symlink"; continue; fi
    [ -d "$_d" ] || { log "warning: skipped $_d: not a directory"; continue; }
    _real="$(readlink -f "$_d" 2>/dev/null || true)"
    if [ -z "$_real" ] || [ "$_real" != "${_d%/}" ]; then
      log "warning: skipped $_d: a parent directory is a symlink"; continue
    fi
    _cur="$(head -n 1 "$_real/$MARKER" 2>/dev/null | head -c 128)"
    if [ -z "$OWNER" ] || [ "$_cur" != "$OWNER" ]; then
      log "warning: skipped $_d: not owned by this store"; continue
    fi
    _clash=""
    _old_ifs="$IFS"; IFS='|'
    for _o in ${FORGE_OS_PROTECT:-}; do
      [ -n "$_o" ] && paths_overlap "$_real" "$_o" && _clash="$_o"
    done
    IFS="$_old_ifs"
    if [ -n "$_clash" ]; then log "warning: skipped $_d: it overlaps $_clash of another store"; continue; fi
    # A nested marker of another store (one Forge may not know about).
    _nested="$(find "$_real" -mindepth 2 -name "$MARKER" -type f 2>/dev/null | head -n 1)"
    if [ -n "$_nested" ]; then log "warning: skipped $_d: it contains data of another store ($_nested)"; continue; fi
    # Never crosses into another filesystem mounted inside the data path.
    if is_mount "$_real"; then find "$_real" -xdev -mindepth 1 -delete 2>/dev/null; else rm -rf --one-file-system "$_real" 2>/dev/null; fi \
      || log "warning: kept files under $_d that are on another filesystem"
    log "deleted the data in $_d"
  done
}

ensure_user() {
  getent group "$SVC_USER" >/dev/null 2>&1 || groupadd --system --gid "$SVC_UID" "$SVC_USER" 2>/dev/null \
    || groupadd --system "$SVC_USER"
  getent passwd "$SVC_USER" >/dev/null 2>&1 || useradd --system --uid "$SVC_UID" --gid "$SVC_USER" \
    --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin "$SVC_USER" 2>/dev/null \
    || useradd --system --gid "$SVC_USER" --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin "$SVC_USER"
}

prepare_data() {
  check_data
  for _d in $DATA_ALL; do
    install -d -m 0750 "$_d"
    if [ "$RUNTIME" = docker ]; then chown "$SVC_UID:$SVC_UID" "$_d"; else chown "$SVC_USER:$SVC_USER" "$_d"; fi
  done
  check_distinct_disks
}

# The fault-tolerance unit of a distributed store is the node: several drives
# of one member may share a disk. RustFS refuses that by default ("local
# erasure endpoints must use distinct physical disks"), so the env file sets
# RUSTFS_UNSAFE_BYPASS_DISK_CHECK=true and the job log warns instead.
SHARED_DISK=0
check_distinct_disks() {
  [ -n "$VOLUMES" ] || return 0
  set -- $DATA_ALL
  [ "$#" -gt 1 ] || return 0
  _seen=""
  _host="$(hostname 2>/dev/null || echo this-node)"
  for _d in $DATA_ALL; do
    _dev="$(stat -c %d "$_d" 2>/dev/null || true)"
    _other=""
    for _e in $_seen; do
      [ "${_e%%=*}" = "$_dev" ] && { _other="${_e#*=}"; break; }
    done
    if [ -n "$_other" ]; then
      SHARED_DISK=1
      log "warning: $_d shares a device with $_other on $_host: fault tolerance is per node"
    else
      _seen="$_seen $_dev=$_d"
    fi
  done
}

# Docker standalone publishes ports; distributed members use the host
# network so the addresses in RUSTFS_VOLUMES are the members' own.
docker_hostnet() {
  [ "$RUNTIME" = docker ] || return 1
  [ -z "$VOLUMES" ] || return 0
  printf '%s\n' "${FORGE_OS_RUN_ARGS:-}" | grep -qx host
}

# The env file holds the credential: root-only, read by systemd or Docker.
write_env() {
  install -d -m 0700 /etc/forge/objstore
  # Data paths are mounted at the same path inside a container.
  _vol="$CONTAINER_DATA"; [ -z "$VOLUMES" ] || _vol="$VOLUMES"
  umask 077
  {
    echo "RUSTFS_ACCESS_KEY=$FORGE_OS_ACCESS_KEY"
    echo "RUSTFS_SECRET_KEY=$FORGE_OS_SECRET_KEY"
    echo "RUSTFS_VOLUMES=$_vol"
    echo "RUSTFS_ADDRESS=0.0.0.0:9000"
    # Members wait for each other's quorum on a cold start.
    [ -z "$VOLUMES" ] || echo "RUSTFS_STARTUP_READINESS_MAX_WAIT_SECS=900"
    # Drives of one member on one disk: the node is the failure unit.
    [ "$SHARED_DISK" != 1 ] || echo "RUSTFS_UNSAFE_BYPASS_DISK_CHECK=true"
    if [ -n "$CONSOLE_PORT" ]; then
      echo "RUSTFS_CONSOLE_ENABLE=true"
      echo "RUSTFS_CONSOLE_ADDRESS=0.0.0.0:9001"
    else
      echo "RUSTFS_CONSOLE_ENABLE=false"
    fi
    echo "RUSTFS_CHECK_UPDATE=false"
    [ -z "${FORGE_OS_REGION:-}" ] || echo "RUSTFS_REGION=$FORGE_OS_REGION"
    # Allow-listed extra variables, validated by the control plane and the node.
    [ -z "${FORGE_OS_EXTRA_ENV:-}" ] || printf '%s\n' "$FORGE_OS_EXTRA_ENV"
  } > "$ENVF.tmp"
  umask 022
  if [ "$RUNTIME" = native ] || docker_hostnet; then
    # Native services and host-network containers listen on the chosen ports.
    sed -i "s/^RUSTFS_ADDRESS=.*/RUSTFS_ADDRESS=0.0.0.0:$S3_PORT/" "$ENVF.tmp"
    [ -z "$CONSOLE_PORT" ] || sed -i "s/^RUSTFS_CONSOLE_ADDRESS=.*/RUSTFS_CONSOLE_ADDRESS=0.0.0.0:$CONSOLE_PORT/" "$ENVF.tmp"
  fi
  chmod 0600 "$ENVF.tmp"
  mv -f "$ENVF.tmp" "$ENVF"
}

# A port another program uses is refused on first install only.
check_ports() {
  command -v ss >/dev/null 2>&1 || return 0
  for _p in "$S3_PORT" $CONSOLE_PORT; do
    [ -z "$(ss -ltnH "sport = :$_p" 2>/dev/null)" ] || die "port $_p is already in use on this node; choose another port"
  done
}

running_state() {
  if [ "$RUNTIME" = docker ]; then
    command -v docker >/dev/null 2>&1 && docker inspect "$UNIT" >/dev/null 2>&1 || { echo not-installed; return; }
    ctr_status "$UNIT"
  else
    [ -f "/etc/systemd/system/$UNIT.service" ] || { echo not-installed; return; }
    if systemctl is-active -q "$UNIT"; then echo running; else echo stopped; fi
  fi
}

# Prints the report line the control plane parses.
report() {
  _state="$(running_state)"
  _ver=""
  if [ "$_state" = running ]; then
    _ver="$(curl -fsS -m 5 "http://127.0.0.1:$S3_PORT/health" 2>/dev/null \
      | sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)"
  fi
  [ -n "$_ver" ] || _ver="$(cat "$BASE/release" 2>/dev/null || true)"
  # Dedicated when every drive is its own mount; capacity summed over the
  # distinct filesystems the drives live on.
  _ded=null; _cap=null; _free=null; _seen=" "
  for _d in $DATA_ALL; do
    [ -d "$_d" ] || continue
    if is_mount "$_d"; then [ "$_ded" = false ] || _ded=true; else _ded=false; fi
    _df="$(df -PB1 "$_d" 2>/dev/null | awk 'NR==2 {print $1" "$2" "$4}')"
    [ -n "$_df" ] || continue
    _fs="${_df%% *}"
    case "$_seen" in *" $_fs "*) continue ;; esac
    _seen="$_seen$_fs "
    _rest="${_df#* }"
    _cap=$(( ${_cap#null} + ${_rest% *} )); _free=$(( ${_free#null} + ${_rest#* } ))
  done
  printf 'forge: objstore: {"state":%s,"version":%s,"dedicated":%s,"capacity_bytes":%s,"free_bytes":%s}\n' \
    "$(json_str "$_state")" "$(json_str "$_ver")" "$_ded" "$_cap" "$_free"
}

# ---------------------------------------------------------------- native
asset_arch() {
  case "$(uname -m)" in
    x86_64|amd64) echo x86_64 ;;
    aarch64|arm64) echo aarch64 ;;
  esac
}

install_binary() {
  command -v curl >/dev/null 2>&1 || os_install curl
  command -v unzip >/dev/null 2>&1 || os_install unzip
  command -v sha256sum >/dev/null 2>&1 || die "sha256sum is missing on this node"
  if [ "$REL" = latest ]; then
    _url="$RELEASES/latest/download"; _suffix=latest
  else
    _url="$RELEASES/download/$REL"; _suffix="v$REL"
  fi
  _asset="rustfs-linux-$(asset_arch)-musl-$_suffix.zip"
  # Disk, not /tmp: /tmp is a small tmpfs on many distributions (Debian 13)
  # and the unpacked binary alone is ~200 MB. Removed on every exit path.
  _tmp="$(mktemp -d /var/tmp/forge-objstore.XXXXXX)"
  trap 'rm -rf "$_tmp"' EXIT
  log "downloading $_asset"
  curl -fsSL --retry 3 -o "$_tmp/$_asset" "$_url/$_asset" || { rm -rf "$_tmp"; die "cannot download $_url/$_asset"; }
  curl -fsSL --retry 3 -o "$_tmp/SHA256SUMS" "$_url/SHA256SUMS" || { rm -rf "$_tmp"; die "cannot download SHA256SUMS"; }
  grep " \*\?$_asset\$" "$_tmp/SHA256SUMS" | head -1 | sed 's/ \*/  /' > "$_tmp/sum"
  [ -s "$_tmp/sum" ] || { rm -rf "$_tmp"; die "$_asset is not listed in SHA256SUMS"; }
  (cd "$_tmp" && sha256sum -c --status sum) || { rm -rf "$_tmp"; die "checksum mismatch for $_asset"; }
  log "checksum verified: $(cut -c1-16 "$_tmp/sum")…"
  unzip -q -o "$_tmp/$_asset" -d "$_tmp/x" </dev/null || { rm -rf "$_tmp"; die "cannot unpack $_asset (disk full?)"; }
  _bin="$(find "$_tmp/x" -type f -name rustfs | head -1)"
  [ -n "$_bin" ] || { rm -rf "$_tmp"; die "no rustfs binary in $_asset"; }
  chmod 0755 "$_bin"
  _ver="$("$_bin" --version 2>/dev/null | grep -o '[0-9][0-9A-Za-z.+-]*' | head -1 || true)"
  [ -n "$_ver" ] || _ver="$REL"
  install -d -m 0755 "$BASE/$_ver"
  install -m 0755 "$_bin" "$BASE/$_ver/rustfs"
  ln -sfn "$BASE/$_ver" "$BASE/current"
  printf '%s\n' "$_ver" > "$BASE/release"
  rm -rf "$_tmp"
  trap - EXIT
  log "installed RustFS $_ver in $BASE/$_ver"
}

write_unit() {
  _pre=""
  if [ "$FORGE_OS_REQUIRE_MOUNT" = 1 ]; then
    for _d in $DATA_ALL; do _pre="${_pre}ExecStartPre=/bin/mountpoint -q $_d
"; done
  fi
  cat > "/etc/systemd/system/$UNIT.service.tmp" <<EOF
# Managed by Orbyn Forge (object store $SLUG). Changes are overwritten.
[Unit]
Description=RustFS object store $SLUG (Orbyn Forge)
After=network-online.target local-fs.target
Wants=network-online.target
RequiresMountsFor=$DATA_ALL

[Service]
Type=simple
User=$SVC_USER
Group=$SVC_USER
EnvironmentFile=$ENVF
$_pre
ExecStart=$BASE/current/rustfs server
Restart=always
RestartSec=10s
TimeoutStopSec=45s
LimitNOFILE=1048576
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=$DATA_ALL

[Install]
WantedBy=multi-user.target
EOF
  mv -f "/etc/systemd/system/$UNIT.service.tmp" "/etc/systemd/system/$UNIT.service"
  systemctl daemon-reload
}

native_install() {
  need_systemd
  ensure_user
  install_binary
  write_unit
  systemctl enable -q "$UNIT"
  systemctl restart "$UNIT"
}

# ---------------------------------------------------------------- docker
# Named volumes the store uses, created when missing. Each line of
# FORGE_OS_VOLUME_CREATE is `[--driver d] [--opt k=v ...] name`, validated
# tokens without spaces or quotes.
ensure_volumes() {
  [ -n "${FORGE_OS_VOLUME_CREATE:-}" ] || return 0
  set -f
  # A here-document, not a pipeline: a failure returns from this shell.
  while IFS= read -r _line; do
    [ -n "$_line" ] || continue
    _name="${_line##* }"
    docker volume inspect "$_name" >/dev/null 2>&1 && continue
    # shellcheck disable=SC2086
    docker volume create $_line >/dev/null || { set +f; log "cannot create volume $_name"; return 1; }
    log "created volume $_name"
  done <<EOF_VOLUMES
$FORGE_OS_VOLUME_CREATE
EOF_VOLUMES
  set +f
}

# docker run with the arguments the control plane rendered (one per line,
# passed without word splitting or globbing).
run_container() {
  _old_ifs="$IFS"; IFS='
'
  set -f
  # shellcheck disable=SC2086
  set -- $FORGE_OS_RUN_ARGS
  IFS="$_old_ifs"; set +f
  docker run -d --name "$UNIT" --env-file "$ENVF" "$@" "$_img" >/dev/null
}

docker_install() {
  need_docker
  _img="${FORGE_OS_IMAGE:-rustfs/rustfs:$REL}"
  log "pulling $_img"
  docker pull -q "$_img" >/dev/null || die "cannot pull $_img"
  ensure_volumes || die "cannot create the store's volumes"
  docker rm -f "$UNIT" >/dev/null 2>&1 || true
  run_container || die "cannot start the container $UNIT from $_img"
  install -d -m 0755 "$BASE"
  docker image inspect -f '{{index .RepoDigests 0}}' "$_img" 2>/dev/null > "$BASE/image" || true
  printf '%s\n' "$REL" > "$BASE/release"
}

# ------------------------------------------------------------ start-up
# How often the engine was restarted since it was started.
engine_restarts() {
  if [ "$RUNTIME" = docker ]; then
    docker inspect -f '{{.RestartCount}}' "$UNIT" 2>/dev/null || echo 0
  else
    systemctl show -p NRestarts --value "$UNIT" 2>/dev/null || echo 0
  fi
}

# The engine's last log lines, into the job output. Lines that contain the
# store's credential are dropped: some engines print it when they start.
engine_log() {
  log "last lines of the RustFS log on $(hostname 2>/dev/null || echo this-node):"
  if [ "$RUNTIME" = docker ]; then
    docker logs --tail 10 "$UNIT" 2>&1 || true
  else
    journalctl -u "$UNIT" -n 10 --no-pager -o cat 2>/dev/null || true
  fi | grep -v -F -e "${FORGE_OS_SECRET_KEY:-forge-no-secret}" -e "${FORGE_OS_ACCESS_KEY:-forge-no-key}" | cut -c1-300
}

# host:port of every member, from the volume list (http://host:port/path).
member_addrs() {
  for _v in $VOLUMES; do
    _a="${_v#*://}"; echo "${_a%%/*}"
  done | sort -u
}

# wait_store <seconds>: until the store answers on its health URL. Fails at
# once, with the engine's log, when RustFS keeps crashing; and says every
# 30 seconds which members it is still waiting for, so a wait is never silent.
wait_store() {
  _url="http://127.0.0.1:$S3_PORT/health"
  _start="$(date +%s)"; _said="$_start"
  until curl -sk -o /dev/null -m 3 "$_url"; do
    _now="$(date +%s)"; _t=$((_now - _start))
    _n="$(engine_restarts)"
    case "$_n" in ''|*[!0-9]*) _n=0 ;; esac
    if [ "$_n" -ge 3 ]; then
      engine_log
      die "RustFS keeps stopping on this node (restarted $_n times); see its log above"
    fi
    if [ "$_t" -ge "$1" ]; then
      engine_log
      if [ -n "$VOLUMES" ]; then
        die "RustFS $SLUG did not answer within $1 s: check that every member is installed and that the members reach each other on port $S3_PORT"
      fi
      die "RustFS did not answer on $_url within $1 s"
    fi
    if [ -n "$VOLUMES" ] && [ $((_now - _said)) -ge 30 ]; then
      _said="$_now"; _down=""
      for _m in $(member_addrs); do
        # Any HTTP answer counts: only a refused or silent port is "down".
        curl -s -o /dev/null -m 3 "http://$_m/" 2>/dev/null || _down="$_down $_m"
      done
      if [ -n "$_down" ]; then
        log "still waiting after $_t s: no answer from$_down"
      else
        log "still waiting after $_t s: every member accepts connections; the store is starting"
      fi
    fi
    sleep 1
  done
}

# ---------------------------------------------------------------- actions
os_install_store() {
  [ -n "$(asset_arch)" ] || die "unsupported architecture $(uname -m)"
  first=1; [ "$(running_state)" = not-installed ] || first=0
  [ "$first" = 0 ] || check_ports
  # The data paths are chowned to the service user: create it first.
  [ "$RUNTIME" = docker ] || ensure_user
  prepare_data
  write_markers
  write_env
  if [ "$RUNTIME" = docker ]; then docker_install; else native_install; fi
  fw_open "$S3_PORT"
  [ -z "$CONSOLE_PORT" ] || fw_open "$CONSOLE_PORT"
  ID="RustFS"
  if [ -n "$VOLUMES" ]; then
    # A member answers once the other members are up too.
    log "waiting for the other members of $SLUG (up to 15 minutes; they install at the same time)"
    wait_store 900
  else
    wait_store 90
  fi
  log "RustFS $SLUG answers on port $S3_PORT"
}

# Recreates the container with new settings, keeping the data. The old
# container is renamed and stopped, not removed, until the new one answers;
# when it does not, the new one is removed and the old one comes back.
os_reconfigure() {
  need_docker
  docker inspect "$UNIT" >/dev/null 2>&1 || die "the container $UNIT does not exist on this node"
  _img="${FORGE_OS_IMAGE:-rustfs/rustfs:$REL}"
  # Pull first: a missing image never takes the store down.
  log "pulling $_img"
  docker pull -q "$_img" >/dev/null || die "cannot pull $_img; the store was not changed"
  _prev="$UNIT-forge-previous"
  docker rm -f "$_prev" >/dev/null 2>&1 || true
  _old_img="$(docker inspect -f '{{.Config.Image}}' "$UNIT" 2>/dev/null || echo unknown)"
  _envbak=""
  if [ -f "$ENVF" ]; then _envbak="$ENVF.forge-previous"; cp -p "$ENVF" "$_envbak"; fi
  docker stop -t 30 "$UNIT" >/dev/null 2>&1 || true
  docker rename "$UNIT" "$_prev" || die "cannot set the old container aside"
  rollback() {
    log "the new container did not come up; restoring the previous one ($_old_img)"
    docker rm -f "$UNIT" >/dev/null 2>&1 || true
    [ -z "$_envbak" ] || mv -f "$_envbak" "$ENVF"
    docker rename "$_prev" "$UNIT" && docker start "$UNIT" >/dev/null
    die "$1; rolled back to the previous container"
  }
  write_env
  ensure_volumes || rollback "cannot create the store's volumes"
  run_container || rollback "cannot start $_img"
  fw_open "$S3_PORT"
  [ -z "$CONSOLE_PORT" ] || fw_open "$CONSOLE_PORT"
  ID="RustFS"
  # A failed health check rolls back instead of dying.
  _start="$(date +%s)"
  until curl -sk -o /dev/null -m 3 "http://127.0.0.1:$S3_PORT/health"; do
    _n="$(engine_restarts)"; case "$_n" in ''|*[!0-9]*) _n=0 ;; esac
    if [ "$_n" -ge 3 ] || [ $(( $(date +%s) - _start )) -ge 90 ]; then
      engine_log
      rollback "RustFS did not answer on port $S3_PORT"
    fi
    sleep 1
  done
  docker rm -f "$_prev" >/dev/null 2>&1 || true
  rm -f "$_envbak"
  install -d -m 0755 "$BASE"
  printf '%s\n' "$REL" > "$BASE/release"
  log "recreated $UNIT from $_img (was $_old_img); data kept"
}

os_uninstall_store() {
  if [ "$RUNTIME" = docker ] && [ "$ADOPTED" = 1 ] && [ "$UNIT" != forge-rustfs ]; then
    # A container Forge adopted but did not create stays as it is.
    log "kept the adopted container $UNIT; Forge no longer manages it"
  elif [ "$RUNTIME" = docker ]; then
    command -v docker >/dev/null 2>&1 && docker rm -f "$UNIT" >/dev/null 2>&1 || true
    # The adopted App Store container's settings (its credential is in the vault).
    [ "$ADOPTED" != 1 ] || rm -f "$LEGACY_ENV"
  elif [ -f "/etc/systemd/system/$UNIT.service" ]; then
    systemctl disable --now -q "$UNIT" 2>/dev/null || true
    rm -f "/etc/systemd/system/$UNIT.service"
    systemctl daemon-reload
  fi
  rm -rf "$BASE"
  rm -f "$ENVF"
  [ "${FORGE_OS_DELETE_DATA:-0}" != 1 ] || delete_data_paths
  fw_close "$S3_PORT"
  [ -z "$CONSOLE_PORT" ] || fw_close "$CONSOLE_PORT"
  # The last store on the node takes the shared directories and service
  # user with it. The user stays while any data path still belongs to it, so
  # kept data keeps a named owner for a later reinstall.
  rmdir /opt/forge/objstore /etc/forge/objstore 2>/dev/null || true
  if [ ! -d /opt/forge/objstore ] && getent passwd "$SVC_USER" >/dev/null 2>&1; then
    _owned=""
    for _d in $DATA_ALL; do
      [ "$(stat -c %U "$_d" 2>/dev/null)" = "$SVC_USER" ] && _owned=1
    done
    if [ -z "$_owned" ]; then
      userdel "$SVC_USER" 2>/dev/null || true
      groupdel "$SVC_USER" 2>/dev/null || true
    fi
  fi
  if [ "${FORGE_OS_DELETE_DATA:-0}" = 1 ]; then log "removed $UNIT and its data"; else log "removed $UNIT; data kept in $DATA_ALL"; fi
}

os_lifecycle() {
  [ "$(running_state)" != not-installed ] || die "object store $SLUG is not installed on this node"
  [ "$1" = stop ] || check_data
  if [ "$RUNTIME" = docker ]; then docker "$1" "$UNIT" >/dev/null; else systemctl "$1" "$UNIT"; fi
  if [ "$1" != stop ]; then ID="RustFS"; wait_http "http://127.0.0.1:$S3_PORT/health" 60; fi
}

# Takes over the App Store container in place: it keeps running untouched,
# and its credential is reported once (the agent takes the line out of the
# output and the control plane seals it in the vault).
# Any RustFS container (D-170): the root credential is read from the
# container's own environment on the node and reported once, like the App
# Store's; allow-listed engine settings are reported for the store record.
os_adopt_container() {
  need_docker
  docker inspect "$UNIT" >/dev/null 2>&1 || die "there is no container $UNIT on this node"
  _envs="$(docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$UNIT")"
  _ak="$(printf '%s\n' "$_envs" | sed -n 's/^RUSTFS_ACCESS_KEY=//p' | tail -1)"
  _sk="$(printf '%s\n' "$_envs" | sed -n 's/^RUSTFS_SECRET_KEY=//p' | tail -1)"
  if [ -z "$_ak" ] && [ -z "$_sk" ]; then
    log "warning: $UNIT sets no credential; it uses the engine's default, change it after adoption"
    _ak=rustfsadmin; _sk=rustfsadmin
  fi
  [ -n "$_ak" ] && [ -n "$_sk" ] || die "the container's credential is incomplete"
  case "$_ak$_sk" in *[!A-Za-z0-9]*) die "the container's keys have characters Forge does not store (letters and digits only)" ;; esac
  [ "$(running_state)" = running ] || docker start "$UNIT" >/dev/null
  ID="RustFS"
  wait_http "http://127.0.0.1:$S3_PORT/health" 60
  printf 'forge: objstore-credential: {"access_key":"%s","secret_key":"%s"}\n' "$_ak" "$_sk"
  _json=""
  for _k in $ENV_ALLOWLIST; do
    _v="$(printf '%s\n' "$_envs" | sed -n "s/^$_k=//p" | tail -1)"
    [ -n "$_v" ] || continue
    _json="$_json${_json:+,}$(json_str "$_k"):$(json_str "$_v")"
  done
  printf 'forge: objstore-adopted-env: {%s}\n' "$_json"
  log "adopted $UNIT"
}

os_adopt() {
  if [ -n "${FORGE_OS_CONTAINER:-}" ] && [ "$UNIT" != forge-rustfs ]; then os_adopt_container; return; fi
  need_docker
  docker inspect "$UNIT" >/dev/null 2>&1 || die "there is no App Store RustFS container ($UNIT) on this node"
  [ -f "$LEGACY_ENV" ] || die "the App Store RustFS settings ($LEGACY_ENV) are missing"
  _ak="$(sed -n 's/^FORGE_PKG_RUSTFS_ACCESS_KEY=//p' "$LEGACY_ENV" | tail -1)"
  _sk="$(sed -n 's/^FORGE_PKG_RUSTFS_SECRET_KEY=//p' "$LEGACY_ENV" | tail -1)"
  [ -n "$_ak" ] && [ -n "$_sk" ] || die "the App Store RustFS keys are missing from $LEGACY_ENV"
  case "$_ak$_sk" in *[!A-Za-z0-9]*) die "the App Store RustFS keys have unexpected characters" ;; esac
  [ "$(running_state)" = running ] || docker start "$UNIT" >/dev/null
  ID="RustFS"
  wait_http "http://127.0.0.1:$S3_PORT/health" 60
  printf 'forge: objstore-credential: {"access_key":"%s","secret_key":"%s"}\n' "$_ak" "$_sk"
  write_markers
  log "adopted $UNIT (data in $DATA)"
}

case "$FORGE_ACTION" in
  adopt) os_adopt ;;
  reconfigure) os_reconfigure ;;
  install)
    [ "$ADOPTED" != 1 ] || die "an adopted App Store container is kept as it is; it cannot be reinstalled"
    os_install_store ;;
  uninstall) os_uninstall_store ;;
  start|stop|restart) os_lifecycle "$FORGE_ACTION" ;;
  # Stores installed before ownership markers get theirs on a status check.
  status) write_markers ;;
  *) die "unknown action $FORGE_ACTION" ;;
esac
report
