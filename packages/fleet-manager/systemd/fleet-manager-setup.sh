#!/usr/bin/env bash
# Install, update and inspect a bare-metal Manifold Guardian (systemd + Nix).
# See ../bare-metal.md. Run as root; with no arguments it shows a menu.
set -Eeuo pipefail
# Without this a failing command inside $(...) does not stop the function there.
shopt -s inherit_errexit
umask 077

repo=github:fedibtc/manifold
image=ghcr.io/fedibtc/manifold-fman
# Production releases have no tags; the Umbrel app's image pin is the release.
pin_url=https://raw.githubusercontent.com/getumbrel/umbrel-apps/master/manifold-fedimint-guardian/docker-compose.yml
cache=https://fedimint.cachix.org
cache_key=fedimint.cachix.org-1:FpJJjy1iPVlvyv4OMiN5y9+/arFLPcnZhZVVCHCDYTs=

# Tests relocate every host path under a scratch prefix.
prefix=${FLEET_MANAGER_SETUP_PREFIX:-}
settle=${FLEET_MANAGER_SETUP_SETTLE:-30}
root=$prefix/opt/fleet-manager
etc=$prefix/etc/fleet-manager
units=$prefix/etc/systemd/system
data=$prefix/var/lib/fleet-manager
backups=$prefix/var/backups/fleet-manager
state=$root/update.state
unit_dir=${unit_dir:-$(dirname "$(readlink -f "$0")")}
service=fleet-manager.service

say() { printf '== %s\n' "$*"; }
die() {
  printf 'fleet-manager-setup: %s\n' "$*" >&2
  exit 1
}
ask() {
  local a
  read -rp "$1${2:+ [$2]}: " a || true
  printf '%s' "${a:-${2-}}"
}
confirm() {
  [[ ${assume_yes:-} == 1 ]] && return 0
  local a
  read -rp "$1 [y/N]: " a || true
  [[ $a == [Yy]* ]]
}
need_root() { [[ $EUID -eq 0 || -n $prefix ]] || die "run as root (sudo)"; }
take_lock() {
  mkdir -p "$prefix/run"
  exec 9>"$prefix/run/fleet-manager-setup.lock"
  flock -n 9 || die "another fleet-manager-setup is running"
}
find_nix() {
  nix=$(command -v nix || echo /nix/var/nix/profiles/default/bin/nix)
  [[ -x $nix ]] || die "Nix is not installed; install it (multi-user) from https://nixos.org/download and re-run"
  # Root is a trusted Nix user; the cache options also cover commits without nixConfig.
  nixf=(--extra-experimental-features 'nix-command flakes' --accept-flake-config
    --option extra-substituters "$cache" --option extra-trusted-public-keys "$cache_key")
}
kv_get() { # file key
  [[ -r $1 ]] || return 0
  sed -n "s/^$2=//p" "$1" | tail -n 1
}
env_get() { kv_get "$etc/fleet-manager.env" "$1"; }
state_get() { kv_get "$state" "$1"; }
installed() {
  local t
  t=$(readlink "$root/current" 2>/dev/null) || return 0
  printf '%s\n' "${t##*/}"
}

# --------------------------------------------------------------- release oracle
# Every lookup is HTTPS-only and bounded in time and size.
fetch() { curl -fsS --proto =https --proto-redir =https --max-time 20 --max-filesize 1000000 "$@"; }

production_pin() {
  local body pins
  body=$(fetch "$pin_url") || die "cannot fetch the production pin from $pin_url"
  ((${#body} <= 1000000)) || die "the production pin source is larger than 1 MB"
  pins=$(grep -oE "$image:[0-9a-f]{40}" <<<"$body" | sort -u) || die "no $image:<commit> in $pin_url"
  [[ $(wc -l <<<"$pins") -eq 1 ]] || die "ambiguous production pin in $pin_url: $(tr '\n' ' ' <<<"$pins")"
  printf '%s\n' "${pins##*:}"
}

published() { # $1=commit: GHCR serves its image, so it went through the publish workflow
  local token code
  token=$(fetch "https://ghcr.io/token?scope=repository:${image#ghcr.io/}:pull" | jq -er .token) ||
    die "cannot get an anonymous GHCR pull token"
  code=$(fetch -o /dev/null -w '%{http_code}' -H @<(printf 'Authorization: Bearer %s\n' "$token") \
    -H 'Accept: application/vnd.oci.image.index.v1+json' \
    "https://ghcr.io/v2/${image#ghcr.io/}/manifests/$1") || true
  [[ $code == 200 ]]
}

compare() { # $1=installed $2=candidate → identical | ahead | behind | diverged
  fetch "https://api.github.com/repos/fedibtc/manifold/compare/$1...$2" | jq -er .status ||
    die "cannot compare $1 with $2 on GitHub"
}

# --------------------------------------------------------------- host artifacts
# A release is one commit's daemon plus that commit's own entrypoint, so the
# launch contract is always the one CI checked against that daemon. The out-link
# is the GC root; `.complete` marks a finished build.
build_release() {
  local rel=$root/releases/$1 src
  [[ -e $rel/.complete ]] && return 0
  install -d -m 0755 "$root" "$root/releases" "$rel"
  say "Building fleet-manager @ $1. On a small machine this can take many hours; keep this session open (tmux). bare-metal.md, Build time, shows how to build it elsewhere instead"
  "$nix" "${nixf[@]}" build -L "$repo/$1#fleet-manager" --out-link "$rel/fleet-manager" </dev/null
  # prefetch, not metadata: metadata may name a source path it never fetched.
  src=$("$nix" "${nixf[@]}" flake prefetch --json "$repo/$1" </dev/null |
    jq -er --arg rev "$1" 'select(.locked.rev == $rev) | .storePath') ||
    die "nix resolved $repo/$1 to a different revision"
  # The entrypoint runs under bash from that commit's own nixpkgs lock: hosts
  # such as NixOS have no bash on the unit's PATH for /usr/bin/env to find.
  "$nix" "${nixf[@]}" build --inputs-from "$repo/$1" 'nixpkgs#bash^out' --out-link "$rel/bash" </dev/null
  sed "1s|^#!.*|#!$rel/bash/bin/bash|" "$src/packages/fleet-manager/entrypoint.sh" >"$rel/entrypoint.sh"
  chmod 0755 "$rel/entrypoint.sh"
  [[ -x $rel/fleet-manager/bin/fleet-manager ]] || die "the build of $1 has no fleet-manager binary"
  touch "$rel/.complete"
}

switch_current() {
  ln -sfn "releases/$1" "$root/.current.tmp"
  mv -fT "$root/.current.tmp" "$root/current"
}

install_units() {
  local u
  install -d -m 0755 "$units"
  for u in fleet-manager.service fleet-manager-update-check.service fleet-manager-update-check.timer; do
    install -m 0644 "$unit_dir/$u" "$units/.$u.tmp"
    mv -f "$units/.$u.tmp" "$units/$u"
  done
  systemctl daemon-reload
}

write_state() { # phase from to backup [was_active]
  printf 'phase=%s\nfrom=%s\nto=%s\nbackup=%s\nwas_active=%s\nat=%s\n' \
    "$1" "$2" "$3" "$4" "${5-}" "$(date -u +%FT%TZ)" >"$state.tmp"
  mv -f "$state.tmp" "$state"
}

# Active for the whole settle window without a restart, then the dashboard answers.
healthy() {
  local restarts bind host
  restarts=$(systemctl show -p NRestarts --value "$service")
  for _ in $(seq "$((settle / 2))"); do
    sleep 2
    systemctl is-active --quiet "$service" || return 1
  done
  systemctl is-active --quiet "$service" || return 1
  [[ $(systemctl show -p NRestarts --value "$service") == "$restarts" ]] || return 1
  bind=$(env_get FLEET_MANAGER_ADMIN_HTTP_BIND)
  host=${bind%:*}
  [[ $host == 0.0.0.0 ]] && host=127.0.0.1
  curl -s --noproxy '*' -o /dev/null --max-time 5 "http://$host:${bind##*:}/"
}

# --------------------------------------------------------------- disks
human() { numfmt --to=iec --suffix=B "$1"; }
fstab=$prefix/etc/fstab

fstab_add() { # one fstab line, added once; the new fstab replaces the old by rename
  grep -qxF "$1" "$fstab" 2>/dev/null && return 0
  local tmp
  tmp=$(mktemp "$fstab.XXXXXX")
  if [[ -e $fstab ]]; then
    cp -p "$fstab" "$fstab.bak-$(date -u +%Y%m%dT%H%M%SZ)"
    cat "$fstab" >"$tmp"
    # A last line without a newline would swallow the new entry.
    [[ ! -s $fstab || -z $(tail -c 1 "$fstab") ]] || echo >>"$tmp"
  fi
  printf '%s\n' "$1" >>"$tmp"
  chmod 0644 "$tmp"
  mv -f "$tmp" "$fstab"
  systemctl daemon-reload
}

mount_path() {
  findmnt -rn "$1" >/dev/null || mount "$1"
  findmnt -rn "$1" >/dev/null || [[ -n $prefix ]] || die "$1 did not mount"
}

# Identity of a block device, including any filesystem on it; formatting
# requires it unchanged since the operator chose the device.
dev_identity() { lsblk -dnbo MAJ:MIN,SIZE,SERIAL,WWN,FSTYPE "$1" | tr -s ' ' _; }

# Unmounted block devices of at least 10 GiB that are not RAID/LVM/LUKS/swap members.
list_unmounted() {
  lsblk -Jpb -o NAME,SIZE,TYPE,FSTYPE,MOUNTPOINT,MODEL | jq -r '
    .. | objects | select(.name? and .size >= 10737418240 and .mountpoint == null and .children == null)
    | select(.type | test("^(disk|part|raid|md|lvm)"))
    | select((.fstype // "") | test("^(linux_raid_member|LVM2_member|swap|crypto_LUKS|vfat|zfs_member|iso9660)$") | not)
    | [.name, .size, (.fstype // "-"), (.model // "-")] | @tsv' | sort -u
}

# An unmounted disk: mounted as is when it already has a Linux filesystem,
# otherwise formatted ext4 (pick_location already had the operator type its path).
prepare_disk() { # device fstype identity → prints its new mount point
  local dev=$1 fs=$2 uuid mp
  # Minutes may pass between the question and this point: the device must still
  # be the one the operator chose, and still unmounted and unused.
  [[ $(dev_identity "$dev") == "$3" ]] || die "$dev changed since you chose it; nothing was formatted"
  list_unmounted | cut -f1 | grep -qxF "$dev" || die "$dev is no longer an unused disk; nothing was formatted"
  if [[ ! $fs =~ ^(ext4|xfs|btrfs)$ ]]; then
    mkfs.ext4 -F -q -L "$(basename "$dev")" "$dev" >&2
    fs=ext4
  fi
  uuid=$(blkid -s UUID -o value "$dev") || true
  [[ -n $uuid ]] || die "no filesystem UUID on $dev"
  mp=$prefix/mnt/$(basename "$dev")
  install -d -m 0755 "$mp"
  fstab_add "UUID=$uuid $mp $fs defaults,noatime,nofail,x-systemd.device-timeout=15 0 2"
  mount_path "$mp" >&2
  printf '%s\n' "$mp"
}

pick_location() { # purpose min-bytes → prints the chosen mount point
  local purpose=$1 min=$2 i=0 choice line tgt fst avail name size model note
  local -a opt=()
  printf '\nWhere should %s live? (needs about %s)\n' "$purpose" "$(human "$min")" >&2
  while read -r tgt _ fst avail; do
    [[ $tgt == /boot* || $tgt == /nix* ]] && continue
    opt+=("M $tgt $avail")
    i=$((i + 1))
    printf '%2d) %-28s %-6s %s free\n' "$i" "$tgt" "$fst" "$(human "$avail")" >&2
  done < <(findmnt -rn -b -o TARGET,SOURCE,FSTYPE,AVAIL -t ext4,xfs,btrfs)
  while IFS=$'\t' read -r name size fst model; do
    [[ -n $name ]] || continue
    opt+=("U $name $fst")
    i=$((i + 1))
    note=" (will be FORMATTED)"
    [[ ! $fst =~ ^(ext4|xfs|btrfs)$ ]] || note=" ($fst, mounted as is)"
    printf '%2d) %-28s unmounted, %s, %s%s\n' "$i" "$name" "$(human "$size")" "$model" "$note" >&2
  done < <(list_unmounted)
  ((i > 0)) || die "no usable filesystem found"
  choice=$(ask "number" 1)
  if [[ ! $choice =~ ^[0-9]+$ ]] || ((choice < 1 || choice > i)); then die "no option '$choice'"; fi
  read -r -a line <<<"${opt[choice - 1]}"
  if [[ ${line[0]} == U ]]; then
    if [[ ! ${line[2]} =~ ^(ext4|xfs|btrfs)$ ]]; then
      printf 'Formatting ERASES everything on %s:\n' "${line[1]}" >&2
      wipefs -n "${line[1]}" >&2 || true
      [[ $(ask "type the device path (${line[1]}) to erase it") == "${line[1]}" ]] ||
        die "not confirmed; ${line[1]} is untouched and nothing changed"
    fi
    printf 'U %s %s %s\n' "${line[1]}" "${line[2]}" "$(dev_identity "${line[1]}")"
    return 0
  fi
  ((line[2] >= min)) || confirm "only $(human "${line[2]}") free on ${line[1]}; use it anyway?" >&2 ||
    die "aborted; nothing changed"
  # Root creates and chowns directories there: refuse a place another user controls.
  [[ -n $prefix ]] || root_only_dir "${line[1]}" ||
    die "${line[1]} or a directory above it is writable by a non-root user; choose another place (nothing changed)"
  printf 'M %s\n' "${line[1]}"
}

# A choice from pick_location ("M <mount>" or "U <device> <fstype>") → its mount
# point, formatting or mounting the disk now.
resolve_location() {
  local kind a b c
  read -r kind a b c <<<"$1"
  case $kind in
  U) prepare_disk "$a" "$b" "$c" ;;
  M) printf '%s\n' "$a" ;;
  esac
}

on_var_lib() { [[ $(findmnt -rn -o TARGET --target "$prefix/var/lib") == "$1" ]]; }

# The data root lives on the chosen filesystem and is bind-mounted at
# /var/lib/fleet-manager; the unit's RequiresMountsFor= then refuses to start
# on an empty directory when that disk is missing.
place_data() { # mount point
  local dir=${1%/}/fleet-manager
  on_var_lib "$1" && return 0
  safe_dir_on "$1" "$dir"
  install -d -m 0700 "$dir" "$data"
  fstab_add "$dir $data none bind,nofail,x-systemd.requires-mounts-for=$1 0 0"
  mount_path "$data"
}

# --------------------------------------------------------------- Bitcoin Core
btc_root=$prefix/opt/bitcoin-core
btc_conf=$prefix/etc/bitcoin/bitcoin.conf
btc_marker=$etc/bitcoin-core.managed
rpc_wait=${FLEET_MANAGER_SETUP_RPC_WAIT:-900}
# Tests run unprivileged and cannot chown. -h: never follow a symlink.
own() { [[ -n $prefix ]] || chown -h "$@"; }

rand_pw() { head -c 32 /dev/urandom | basenc --base64url | tr -d '=\n'; }

rpcauth_for() { # user password → the rpcauth= value (as share/rpcauth/rpcauth.py)
  local salt hash
  salt=$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n')
  hash=$(printf '%s' "$2" | openssl dgst -sha256 -hmac "$salt" -r | cut -d' ' -f1)
  printf '%s:%s$%s\n' "$1" "$salt" "$hash"
}

# Replaces that user's rpcauth line, above the first [section] so it applies to
# every network; a dated backup stays beside the file. The edit runs as the
# file's owner, so a config in a directory the node's user controls cannot steer
# a root write elsewhere; a root-owned config must sit in a root-only directory.
# shellcheck disable=SC2016 # an awk program, expanded by awk
rpcauth_awk='
  $0 ~ ("^[[:space:]]*rpcauth=" user ":") { next }
  !done && /^[[:space:]]*\[/ { print line; done = 1 }
  { print }
  END { if (!done) print line }'
# Every directory from / down to $1 is root's and not group/other-writable, so
# nobody else can swap a path component between the check and the write.
root_only_dir() {
  local d
  d=$(realpath -e "$1") || return 1
  while :; do
    [[ -n $(find "$d" -maxdepth 0 -user root ! -perm /022) ]] || return 1
    [[ $d == / ]] && return 0
    d=$(dirname "$d")
  done
}
root_only_path() { root_only_dir "$(dirname "$(realpath -e "$1")")"; }

# A directory the installer creates on a chosen filesystem: that filesystem and
# everything above it must be root-only and the target no symlink, so nobody
# else can redirect the mkdir or chown.
safe_dir_on() { # parent dir
  [[ ! -L $2 ]] || die "$2 is a symlink; refusing to use it"
  [[ -n $prefix ]] || root_only_dir "$1" ||
    die "$1 or a directory above it is writable by other users; choose another place"
}

set_rpcauth() { # conf user value
  local conf=$1 owner
  local -a as_owner=()
  [[ -f $conf && ! -L $conf ]] || die "$conf is not a regular file; add the rpcauth line yourself"
  owner=$(stat -c %U "$conf")
  if [[ -z $prefix && $owner == root ]]; then
    root_only_path "$conf" ||
      die "$conf is root's but a directory above it is writable by others; add the rpcauth line yourself"
  elif [[ -z $prefix ]]; then
    as_owner=(runuser -u "$owner" --)
  fi
  # shellcheck disable=SC2016 # runs as the owner, with its own arguments
  "${as_owner[@]}" bash -c '
    set -euo pipefail
    conf=$1
    cp -p "$conf" "$conf.bak-$(date -u +%Y%m%dT%H%M%SZ)"
    tmp=$(mktemp "$conf.XXXXXX")
    awk -v user="$2" -v line="$3" "$4" "$conf" >"$tmp"
    chmod --reference="$conf" "$tmp"
    mv -f "$tmp" "$conf"' _ "$conf" "$2" "rpcauth=$3" "$rpcauth_awk"
}

# Credentials go to a loopback port only while Bitcoin Core itself holds it:
# during a restart another local user could bind the port and collect them.
# https endpoints are identified by TLS instead. 0 = ok, 1 = held by another
# process, 2 = nothing listening yet.
rpc_listener_ok() { # url
  local port pids pid
  [[ $1 =~ ^http://(127\.0\.0\.1|localhost)(:([0-9]+))? ]] || return 0
  port=${BASH_REMATCH[3]:-80}
  pids=$(ss -Hltnp "sport = :$port" | grep -o 'pid=[0-9]*' | cut -d= -f2 | sort -u) || true
  [[ -n $pids ]] || return 2
  # The executable itself must be a root-owned bitcoind in a root-only directory
  # (a process can rename itself, and a user can run a binary of their own), run
  # by the node's user; with that user unknown nothing is trusted.
  [[ -n ${btc_uid-} ]] || return 1
  local exe
  for pid in $pids; do
    exe=$(readlink "/proc/$pid/exe" 2>/dev/null) || return 1
    [[ $exe == */bitcoind ]] || return 1
    [[ -n $prefix ]] || { [[ $(stat -c %U "$exe") == root ]] && root_only_path "$exe"; } || return 1
    [[ $(stat -c %u "/proc/$pid") == "$btc_uid" ]] || return 1
  done
}

rpc_call() { # url user pass method → body, then the HTTP status on the last line
  local held=0
  rpc_listener_ok "$1" || held=$?
  case $held in
  1) printf '\n999' && return 0 ;;
  2) printf '\n000' && return 0 ;;
  esac
  curl -sS --noproxy '*' --proto =http,https --max-time 10 -w '\n%{http_code}' -K <(printf 'user = "%s:%s"\n' "$2" "$3") \
    -H 'content-type: text/plain' \
    --data-binary "{\"jsonrpc\":\"1.0\",\"id\":\"setup\",\"method\":\"$4\",\"params\":[]}" "$1" 2>/dev/null ||
    printf '\n000'
}

# Prints the chain once RPC answers. 1 = credentials refused, 2 = no answer in
# time, 3 = the port is held by a process that is not Bitcoin Core.
# A node that is starting (unreachable, or error -28 while it loads) is waited for.
wait_rpc() { # url user pass seconds
  local reply code body
  for _ in $(seq "$(($4 / 5 + 1))"); do
    reply=$(rpc_call "$1" "$2" "$3" getblockchaininfo)
    code=${reply##*$'\n'}
    body=${reply%$'\n'*}
    case $code in
    200) jq -er .result.chain <<<"$body" && return 0 ;;
    401 | 403) return 1 ;;
    999) return 3 ;;
    esac
    [[ $code == 000 || $(jq -r '.error.code // empty' <<<"$body" 2>/dev/null) == -28 ]] || return 2
    sleep 5
  done
  return 2
}

latest_btc() {
  fetch https://api.github.com/repos/bitcoin/bitcoin/releases/latest | jq -er '.tag_name | ltrimstr("v")' ||
    die "cannot look up the latest Bitcoin Core release"
}

# Big downloads: still HTTPS-only, with room for the release archive.
download() { curl -fsS --proto =https --proto-redir =https --max-time 1800 --max-filesize 300000000 "$@"; }

# Accepts a release only when at least 3 Bitcoin Core builders (guix.sigs) signed
# its SHA256SUMS and the archive matches its line there.
btc_fetch_verified() { # version → $btc_root/bitcoin-$version
  local v=$1 arch tarball base tmp good
  [[ -x $btc_root/bitcoin-$v/bin/bitcoind ]] && return 0
  case $(uname -m) in
  x86_64 | aarch64) arch=$(uname -m) ;;
  *) die "Bitcoin Core publishes no Linux release for $(uname -m)" ;;
  esac
  tarball=bitcoin-$v-$arch-linux-gnu.tar.gz
  base=https://bitcoincore.org/bin/bitcoin-core-$v
  tmp=$(mktemp -d)
  say "Downloading Bitcoin Core $v and checking its signatures"
  download -o "$tmp/$tarball" "$base/$tarball" || die "cannot download $base/$tarball"
  download -o "$tmp/SHA256SUMS" "$base/SHA256SUMS" || die "cannot download $base/SHA256SUMS"
  download -o "$tmp/SHA256SUMS.asc" "$base/SHA256SUMS.asc" || die "cannot download $base/SHA256SUMS.asc"
  git clone -q --depth 1 https://github.com/bitcoin-core/guix.sigs "$tmp/guix.sigs" ||
    die "cannot fetch the builder keys from github.com/bitcoin-core/guix.sigs"
  install -d -m 0700 "$tmp/gnupg"
  gpg -q --homedir "$tmp/gnupg" --import "$tmp"/guix.sigs/builder-keys/*.gpg 2>/dev/null || true
  # Distinct keys with a GOODSIG: one key signing twice counts once, and gpg
  # reports expired or revoked keys as EXPKEYSIG/REVKEYSIG, never GOODSIG.
  good=$(gpg --homedir "$tmp/gnupg" --status-fd 1 --verify "$tmp/SHA256SUMS.asc" "$tmp/SHA256SUMS" 2>/dev/null |
    awk '$1 == "[GNUPG:]" && $2 == "GOODSIG" { print $3 }' | sort -u | wc -l) || true
  ((${good:-0} >= 3)) || {
    rm -rf "$tmp"
    die "only ${good:-0} distinct builders signed the Bitcoin Core $v SHA256SUMS (need 3); nothing installed"
  }
  (cd "$tmp" && grep -E "^[0-9a-f]{64}  $tarball\$" SHA256SUMS | sha256sum -c --status) || {
    rm -rf "$tmp"
    die "$tarball does not match its signed SHA256SUMS; nothing installed"
  }
  install -d -m 0755 "$btc_root"
  tar --no-same-owner -xzf "$tmp/$tarball" -C "$btc_root"
  rm -rf "$tmp"
  [[ -x $btc_root/bitcoin-$v/bin/bitcoind ]] || die "the Bitcoin Core $v archive has no bin/bitcoind"
}

btc_switch() { # version
  ln -sfn "bitcoin-$1" "$btc_root/.current.tmp"
  mv -fT "$btc_root/.current.tmp" "$btc_root/current"
}

btc_install() { # A Bitcoin Core of our own: user, datadir, config, unit, marker.
  btc_fetch_verified "$btc_version"
  btc_switch "$btc_version"
  id bitcoin &>/dev/null ||
    useradd --system --user-group --no-create-home --home-dir "$btc_datadir" \
      --shell "$(command -v nologin || echo /usr/sbin/nologin)" bitcoin
  btc_uid=$(id -u bitcoin 2>/dev/null) || true
  install -d -m 0700 "$btc_datadir"
  own bitcoin:bitcoin "$btc_datadir"
  install -d -m 0755 "$(dirname "$btc_conf")"
  {
    printf 'chain=%s\nserver=1\ndisablewallet=1\n' "$want"
    ((btc_prune == 0)) || printf 'prune=%s\n' "$btc_prune"
    printf 'rpcauth=%s\n' "$(rpcauth_for "$user" "$pass")"
  } >"$btc_conf.tmp"
  chmod 0640 "$btc_conf.tmp"
  own root:bitcoin "$btc_conf.tmp"
  mv -f "$btc_conf.tmp" "$btc_conf"
  install -d -m 0755 "$units"
  cat >"$units/.bitcoind.service.tmp" <<EOF
# Installed by fleet-manager-setup; update it with: fleet-manager-setup update-bitcoin-core
[Unit]
Description=Bitcoin Core
After=network-online.target
Wants=network-online.target
RequiresMountsFor=$btc_datadir

[Service]
User=bitcoin
Group=bitcoin
ExecStart=$btc_root/current/bin/bitcoind -conf=$btc_conf -datadir=$btc_datadir -printtoconsole=0
Restart=on-failure
TimeoutStopSec=600
NoNewPrivileges=yes
ProtectSystem=full
PrivateTmp=yes

[Install]
WantedBy=multi-user.target
EOF
  chmod 0644 "$units/.bitcoind.service.tmp"
  mv -f "$units/.bitcoind.service.tmp" "$units/bitcoind.service"
  install -d -m 0750 "$etc"
  printf 'version=%s\ndatadir=%s\nconf=%s\n' "$btc_version" "$btc_datadir" "$btc_conf" >"$btc_marker"
  systemctl daemon-reload
  systemctl enable --now bitcoind.service
}

# Finds a running bitcoind: its pid, datadir and config file (argv is read, never printed).
btc_detect() {
  local pids argv
  pids=$(pgrep -x bitcoind) || return 1
  btc_count=$(wc -l <<<"$pids")
  ((btc_count == 1)) || [[ $(for p in $pids; do stat -c %u "/proc/$p"; done | sort -u | wc -l) == 1 ]] ||
    die "several bitcoind processes run as different users; stop the extra ones and re-run Install"
  # The node is trusted only as the main process of a system service: a process
  # merely inside one (a user's cron job sits in cron.service) does not count.
  local p unit
  btc_pid=$(head -n 1 <<<"$pids") btc_uid="" btc_unit=""
  for p in $pids; do
    unit=$(ps -o unit= -p "$p" | tr -d ' ')
    if [[ $unit == *.service && $unit != user@* && $(systemctl show -p MainPID --value "$unit") == "$p" ]]; then
      btc_pid=$p btc_uid=$(stat -c %u "/proc/$p") btc_unit=$unit
      break
    fi
  done
  argv=$(tr '\0' '\n' <"/proc/$btc_pid/cmdline")
  # Credentials on the command line win over the config file.
  btc_argv_auth=no
  ! grep -qE '^-(rpcauth|rpcuser|rpcpassword)=' <<<"$argv" || btc_argv_auth=yes
  btc_found_datadir=$(sed -n 's/^-datadir=//p' <<<"$argv" | tail -n 1)
  [[ -n $btc_found_datadir ]] ||
    btc_found_datadir=$(getent passwd "$(stat -c %U "/proc/$btc_pid")" | cut -d: -f6)/.bitcoin
  btc_found_conf=$(sed -n 's/^-conf=//p' <<<"$argv" | tail -n 1)
  btc_found_conf=${btc_found_conf:-bitcoin.conf}
  [[ $btc_found_conf == /* ]] || btc_found_conf=$btc_found_datadir/$btc_found_conf
}

# Saved credentials still work unless Bitcoin Core refuses them (a node that is
# down or starting does not count as a refusal). Read-only.
creds_work() {
  [[ -n $user && -n $pass ]] || return 1
  local rc=0
  wait_rpc "$url" "$user" "$pass" 10 >/dev/null || rc=$?
  ((rc != 3)) || die "the RPC port in $url is held by a process that is not Bitcoin Core; the credentials were not sent"
  ((rc != 1)) || {
    say "Bitcoin Core refuses the saved RPC user $user; it will be replaced"
    return 1
  }
}

# Questions only: decides how the guardian reaches Bitcoin Core (btc_mode) and
# sets url/user/pass. Nothing on the host changes here.
btc_plan() {
  local v port need
  url=$(env_get FLEET_MANAGER_BITCOIND_URL)
  user=$(env_get FLEET_MANAGER_BITCOIND_USERNAME)
  pass=$(env_get FLEET_MANAGER_BITCOIND_PASSWORD)
  if [[ -r $btc_marker ]]; then
    btc_uid=$(id -u bitcoin 2>/dev/null) || true
    say "Using the Bitcoin Core this installer manages ($(kv_get "$btc_marker" version))"
    url=${url:-http://127.0.0.1:$default_port}
    # An install interrupted before saving its credentials, or refused ones: re-key.
    creds_work && btc_mode=managed || btc_mode=rekey user=manifold pass=$(rand_pw)
    return 0
  fi
  if btc_detect; then
    say "Bitcoin Core is running (pid $btc_pid, config $btc_found_conf)"
    port=$(kv_get "$btc_found_conf" rpcport)
    url=$(ask "Bitcoin Core RPC URL" "${url:-http://127.0.0.1:${port:-$default_port}}")
    valid_rpc_url "$url"
    if creds_work; then
      btc_mode=keep
      say "Keeping the RPC user $user from the last install"
      return 0
    fi
    if ((btc_count > 1)); then
      say "$btc_count bitcoind processes are running; give the credentials of the one at $url"
    elif [[ $btc_argv_auth == yes ]]; then
      say "This node takes RPC credentials on its command line, so its config is not edited"
    elif confirm "Add a new rpcauth user \"manifold\" to $btc_found_conf and restart Bitcoin Core to load it? This restarts your node"; then
      [[ -n $btc_unit ]] ||
        die "Bitcoin Core is not the main process of a systemd service, so it cannot be restarted from here; add an rpcauth user yourself and re-run Install"
      btc_mode=rpcauth user=manifold pass=$(rand_pw)
      return 0
    fi
    btc_mode=manual
    user=$(ask "Bitcoin Core rpcauth user" manifold)
    valid "RPC user" "$user" '^[A-Za-z0-9._-]{1,64}$' "A-Z a-z 0-9 . _ -"
    read -rsp "Bitcoin Core rpcauth password: " pass || true
    echo >&2
    [[ $pass =~ ^[A-Za-z0-9._~+/=-]{1,1024}$ ]] ||
      die "RPC password is not accepted; allowed: A-Z a-z 0-9 . _ ~ + / = - (rpcauth.py passwords qualify)"
    return 0
  fi
  systemctl cat bitcoind.service >/dev/null 2>&1 &&
    die "bitcoind.service exists but Bitcoin Core is not running; start it and re-run Install"
  confirm "No Bitcoin Core is running here. Install it (verified download, its own user and service)?" ||
    die "the guardian needs Bitcoin Core on the $want network; install it and re-run"
  btc_mode=install
  btc_version=$(ask "Bitcoin Core version" "$(latest_btc)")
  valid "Bitcoin Core version" "$btc_version" '^[0-9]+\.[0-9]+(\.[0-9]+)?$' "a release number such as 31.1"
  btc_prune=0 need=$((5 * 1024 ** 3))
  if [[ $want == main ]]; then
    if confirm "Keep the full blockchain (about 800 GB)? No prunes it to 10 GB"; then
      need=$((900 * 1024 ** 3))
    else
      btc_prune=10000 need=$((30 * 1024 ** 3))
    fi
  fi
  btc_location=$(pick_location "the blockchain" "$need")
  url=http://127.0.0.1:$default_port user=manifold pass=$(rand_pw)
}

# Carries out btc_plan, then requires RPC to answer on the right chain.
btc_apply() {
  local rc=0 mp
  case $btc_mode in
  rpcauth)
    set_rpcauth "$btc_found_conf" "$user" "$(rpcauth_for "$user" "$pass")"
    say "Added the rpcauth user $user; restarting $btc_unit"
    systemctl restart "$btc_unit"
    ;;
  install)
    mp=$(resolve_location "$btc_location")
    if on_var_lib "$mp"; then btc_datadir=$prefix/var/lib/bitcoind; else btc_datadir=${mp%/}/bitcoin; fi
    safe_dir_on "$(dirname "$btc_datadir")" "$btc_datadir"
    btc_install
    ;;
  rekey)
    set_rpcauth "$btc_conf" "$user" "$(rpcauth_for "$user" "$pass")"
    say "New RPC credentials for the managed Bitcoin Core; restarting it"
    systemctl restart bitcoind.service
    ;;
  managed) systemctl start bitcoind.service ;;
  esac
  say "Waiting for Bitcoin Core RPC at $url"
  chain=$(wait_rpc "$url" "$user" "$pass" "$rpc_wait") || rc=$?
  case $rc in
  0) ;;
  1) die "Bitcoin Core RPC at $url refused these credentials" ;;
  3) die "the RPC port in $url is held by a process that is not Bitcoin Core; the credentials were not sent" ;;
  *) die "Bitcoin Core RPC at $url did not answer within $((rpc_wait / 60)) min; re-run Install once it is up" ;;
  esac
  [[ $chain == "$want" ]] || die "Bitcoin Core is on '$chain'; the $envname environment needs '$want'"
  [[ $btc_mode != install ]] ||
    say "Bitcoin Core $btc_version started; its first sync can take days, and seats only work once it has synced"
}

cmd_update_btc() {
  need_root
  take_lock
  [[ -r $btc_marker ]] ||
    die "this Bitcoin Core was not installed by fleet-manager-setup; update it the way you installed it"
  local cur v rc=0 reply
  btc_uid=$(id -u bitcoin 2>/dev/null) || true
  cur=$(kv_get "$btc_marker" version)
  v=${1:-$(latest_btc)}
  valid "Bitcoin Core version" "$v" '^[0-9]+\.[0-9]+(\.[0-9]+)?$' "a release number such as 31.1"
  if [[ $v == "$cur" ]]; then
    say "Bitcoin Core is up to date: $cur"
    return 0
  fi
  [[ $(printf '%s\n%s\n' "$cur" "$v" | sort -V | tail -n 1) == "$v" ]] ||
    die "Bitcoin Core $v is older than the installed $cur; downgrades are not supported"
  confirm "Update Bitcoin Core $cur -> $v (restarts the node; the guardian keeps running)?" || die "aborted"
  btc_fetch_verified "$v"
  btc_switch "$v"
  systemctl restart bitcoind.service
  wait_rpc "$(env_get FLEET_MANAGER_BITCOIND_URL)" "$(env_get FLEET_MANAGER_BITCOIND_USERNAME)" \
    "$(env_get FLEET_MANAGER_BITCOIND_PASSWORD)" "$rpc_wait" >/dev/null || rc=$?
  ((rc != 3)) || die "the RPC port is held by a process that is not Bitcoin Core; the credentials were not sent"
  ((rc == 0)) || die "Bitcoin Core $v did not answer RPC; see journalctl -u bitcoind. The previous release is in $btc_root/bitcoin-$cur"
  reply=$(rpc_call "$(env_get FLEET_MANAGER_BITCOIND_URL)" "$(env_get FLEET_MANAGER_BITCOIND_USERNAME)" \
    "$(env_get FLEET_MANAGER_BITCOIND_PASSWORD)" getnetworkinfo)
  [[ ${reply%$'\n'*} =~ \"subversion\":\"/Satoshi:${v//./\\.}(\.0)?/ ]] ||
    die "Bitcoin Core answers but does not report version $v; see journalctl -u bitcoind"
  printf 'version=%s\ndatadir=%s\nconf=%s\n' "$v" "$(kv_get "$btc_marker" datadir)" "$(kv_get "$btc_marker" conf)" \
    >"$btc_marker.tmp"
  mv -f "$btc_marker.tmp" "$btc_marker"
  local r
  for r in "$btc_root"/bitcoin-*; do
    [[ -e $r ]] || continue
    [[ ${r##*/} == "bitcoin-$cur" || ${r##*/} == "bitcoin-$v" ]] || rm -rf "$r"
  done
  say "Updated Bitcoin Core $cur -> $v; the previous release stays in $btc_root/bitcoin-$cur"
}

# --------------------------------------------------------------- install
valid() { # name value regex allowed
  [[ $2 =~ $3 ]] || die "$1 '$2' is not accepted; allowed: $4"
}

# RPC sends the credentials with every request: plain HTTP only on this host.
valid_rpc_url() {
  valid "RPC URL" "$1" '^https?://[A-Za-z0-9.:-]+(/[A-Za-z0-9._/-]*)?$' "http(s)://host:port"
  local loopback='^http://(127\.0\.0\.1|localhost)(:[0-9]+)?(/|$)'
  [[ $1 == https://* || $1 =~ $loopback ]] ||
    die "RPC URL '$1' would send the credentials unencrypted; use http only on 127.0.0.1, or https"
  [[ ${envname-} != production || $1 =~ $loopback ]] ||
    die "production needs Bitcoin Core on this host (production-releases.md); use http://127.0.0.1:<port>"
}

cmd_install() {
  need_root
  take_lock
  find_nix
  local v esplora bind cur commit pin data_location="" data_mp
  v=$(systemctl --version | awk 'NR == 1 { print $2 }')
  ((${v%%.*} >= 247)) || die "systemd $v is too old; the unit needs 247 or later (ProtectProc=)"

  # Every question first: apart from its lock in /run, the host changes only
  # after the last answer (reads such as an RPC credential check are fine).
  v=$(env_get FLEET_MANAGER_MANIFOLD_ENVIRONMENT)
  envname=$(ask "Manifold environment (production, staging, development)" "${v:-production}")
  case $envname in
  production) want=main default_port=8332 ;;
  staging) want=signet default_port=38332 ;;
  development) want=regtest default_port=18443 ;;
  *) die "unknown Manifold environment '$envname'" ;;
  esac
  v=$(env_get FLEET_MANAGER_ADMIN_HTTP_BIND)
  bind=$(ask "Dashboard bind address (keep it off the public internet)" "${v:-127.0.0.1:8482}")
  valid "dashboard bind" "$bind" '^[0-9.]+:[0-9]{1,5}$' "IPv4:port"
  btc_plan
  v=$(env_get FLEET_MANAGER_ESPLORA_URL)
  esplora=$(ask "Esplora fallback URL (blank for none; recommended with a pruned node)" "$v")
  [[ -z $esplora ]] || valid "Esplora URL" "$esplora" '^https://[A-Za-z0-9.:-]+(/[A-Za-z0-9._/-]*)?$' "https://host/path"
  if [[ -z $(ls -A "$data" 2>/dev/null) ]] && ! grep -qs " $data " "$fstab"; then
    data_location=$(pick_location "the guardian's data" $((10 * 1024 ** 3)))
  else
    say "The guardian's data stays where it is ($data)"
  fi
  cur=$(installed)
  if [[ -n $cur ]]; then
    commit=$cur
    say "Keeping the installed release $cur (use update to change it)"
  else
    pin=$(production_pin)
    commit=$(ask "Manifold commit to install (default: the production release)" "$pin")
    valid "commit" "$commit" '^[0-9a-f]{40}$' "a full 40-hex commit"
    published "$commit" || die "$commit has no published $image image"
  fi

  btc_apply
  id fleet-manager &>/dev/null ||
    useradd --system --user-group --no-create-home --home-dir /var/lib/fleet-manager \
      --shell "$(command -v nologin || echo /usr/sbin/nologin)" fleet-manager
  # The service user may enter /etc/fleet-manager only to read its dashboard
  # password; the environment file, with the RPC password, stays root-only and is
  # read by systemd before it drops privileges.
  install -d -m 0750 "$etc"
  own root:fleet-manager "$etc"
  {
    printf 'FLEET_MANAGER_MANIFOLD_ENVIRONMENT=%s\n' "$envname"
    printf 'FLEET_MANAGER_BITCOIND_URL=%s\n' "$url"
    printf 'FLEET_MANAGER_BITCOIND_USERNAME=%s\n' "$user"
    printf 'FLEET_MANAGER_BITCOIND_PASSWORD=%s\n' "$pass"
    [[ -z $esplora ]] || printf 'FLEET_MANAGER_ESPLORA_URL=%s\n' "$esplora"
    printf 'FLEET_MANAGER_ADMIN_HTTP_BIND=%s\n' "$bind"
    printf 'FLEET_MANAGER_ADMIN_HTTP_AUTH=password\n'
  } >"$etc/.env.tmp"
  mv -f "$etc/.env.tmp" "$etc/fleet-manager.env"
  if [[ ! -s $etc/admin-password ]]; then
    head -c 24 /dev/urandom | base64 >"$etc/.admin-password.tmp"
    mv -f "$etc/.admin-password.tmp" "$etc/admin-password"
  fi
  # The daemon refuses a password file with any group or other bits.
  chmod 0400 "$etc/admin-password"
  own fleet-manager:fleet-manager "$etc/admin-password"
  if [[ -n $data_location ]]; then
    data_mp=$(resolve_location "$data_location")
    place_data "$data_mp"
  fi

  build_release "$commit"
  switch_current "$commit"
  # The update-check timer runs this script from a GC-rooted copy.
  local self
  self=$(readlink -f "$0")
  self=${self%/bin/*}
  if [[ $self == /nix/store/* ]]; then
    "$nix" "${nixf[@]}" build --out-link "$root/setup" "$self"
    install -d -m 0755 "$prefix/usr/local/sbin"
    ln -sfn "$root/setup/bin/fleet-manager-setup" "$prefix/usr/local/sbin/fleet-manager-setup"
  else
    say "Not running from the Nix store: the update-check timer needs $root/setup (install with nix run)"
  fi
  install_units
  systemctl enable "$service" fleet-manager-update-check.timer
  systemctl restart "$service"
  systemctl start fleet-manager-update-check.timer
  healthy || die "fleet-manager did not stay up; see: journalctl -u fleet-manager"

  cat <<EOF
== Manifold Guardian $commit is running
  dashboard  http://$bind/   password: sudo cat /etc/fleet-manager/admin-password
  seats      open UDP 30000-30031 inbound (iroh); never publish the dashboard
  logs       journalctl -fu fleet-manager
  next       open the dashboard and follow the onboarding (bare-metal.md)
EOF
}

# --------------------------------------------------------------- update
on_update_error() {
  trap - ERR
  if [[ $phase == stopped ]]; then
    rm -f "$bak.partial"
    [[ $(installed) == "$cur" ]] || switch_current "$cur"
    local again=""
    if [[ $was_active == yes ]]; then
      systemctl start "$service"
      again=" and running again"
    fi
    write_state aborted "$cur" "$pin" ""
    die "update aborted before the switch; $cur is unchanged$again"
  fi
  systemctl stop "$service" || true
  write_state failed "$cur" "$pin" "$bak"
  die "$pin did not stay up and is stopped. Backup of the stopped $cur data root: $bak. Previous release: $root/releases/$cur. Restore steps: bare-metal.md"
}

cmd_update() {
  need_root
  take_lock
  find_nix
  local status need free
  cur=$(installed)
  [[ -n $cur ]] || die "nothing is installed; run install first"
  # A run that died after stopping (power loss, kill -9) left the old release
  # down; bring it back before anything here can fail, the network included.
  if [[ $(state_get phase) == stopping && $(state_get from) == "$cur" ]]; then
    rm -f "$(state_get backup).partial"
    if [[ $(state_get was_active) == yes ]] && ! systemctl is-active --quiet "$service"; then
      systemctl start "$service"
    fi
    write_state aborted "$cur" "$(state_get to)" ""
    say "Recovered an interrupted update; $cur is unchanged"
  fi
  pin=${1:-$(production_pin)}
  valid "commit" "$pin" '^[0-9a-f]{40}$' "a full 40-hex commit"
  if [[ $pin == "$cur" ]]; then
    local ph from
    ph=$(state_get phase)
    from=$(state_get from)
    bak=$(state_get backup)
    # An update to this release that never verified is finished here, not skipped.
    if [[ $ph != starting && $ph != failed ]] && systemctl is-active --quiet "$service"; then
      say "Up to date: $cur"
      return 0
    fi
    say "Starting $cur and waiting for it to stay up"
    write_state starting "$from" "$cur" "$bak"
    systemctl start "$service"
    if ! healthy; then
      systemctl stop "$service" || true
      write_state failed "$from" "$cur" "$bak"
      die "$cur did not stay up and is stopped; see: journalctl -u fleet-manager"
    fi
    write_state "done" "$from" "$cur" "$bak"
    say "$cur is running"
    return 0
  fi
  published "$pin" || die "$pin has no published $image image"
  status=$(compare "$cur" "$pin")
  [[ $status == ahead ]] || die "$pin is $status relative to the installed $cur; only newer releases are installed"
  confirm "Update $cur -> $pin (stops the guardian for a backup and the switch)?" || die "aborted"

  build_release "$pin"
  install -d -m 0700 "$backups"
  need=$(du -sb "$data" | cut -f1)
  free=$(df -B1 --output=avail "$backups" | tail -n 1 | tr -d ' ')
  ((free > need * 2)) || die "not enough space in $backups to back up $data"

  was_active=no
  systemctl is-active --quiet "$service" && was_active=yes
  bak=$backups/$cur-$(date -u +%Y%m%dT%H%M%SZ).tar
  write_state stopping "$cur" "$pin" "$bak" "$was_active"
  phase=stopped
  trap on_update_error ERR
  systemctl stop "$service"
  # admin.sock is recreated at start; tar skips sockets anyway, so say nothing about it.
  tar --numeric-owner --warning=no-file-ignored -C "$(dirname "$data")" -cf "$bak.partial" "$(basename "$data")"
  tar -tf "$bak.partial" >/dev/null
  sync "$bak.partial"
  mv "$bak.partial" "$bak"

  switch_current "$pin"
  install_units
  write_state starting "$cur" "$pin" "$bak"
  # From here the new daemon may migrate the data root: no automatic rollback.
  phase=started
  systemctl start "$service"
  healthy || on_update_error
  trap - ERR
  write_state "done" "$cur" "$pin" "$bak"
  local r
  for r in "$root"/releases/*; do
    [[ -e $r ]] || continue
    [[ ${r##*/} == "$cur" || ${r##*/} == "$pin" ]] || rm -rf "$r"
  done
  say "Updated $cur -> $pin; backup $bak"
}

# --------------------------------------------------------------- status, check
cmd_status() {
  need_root
  if [[ -r $btc_marker ]]; then btc_uid=$(id -u bitcoin 2>/dev/null) || true; else btc_detect || true; fi
  local avail=$prefix/var/lib/fleet-manager-setup/available
  printf 'installed    %s\n' "$(installed)"
  [[ -r $avail ]] && printf 'last check   %s\n' "$(cat "$avail")"
  printf 'service      %s\n' "$(systemctl is-active "$service" || true)"
  [[ -r $state ]] && printf 'last update  %s\n' "$(tr '\n' ' ' <"$state")"
  printf 'dashboard    http://%s/\n' "$(env_get FLEET_MANAGER_ADMIN_HTTP_BIND)"
  local reply
  reply=$(rpc_call "$(env_get FLEET_MANAGER_BITCOIND_URL)" "$(env_get FLEET_MANAGER_BITCOIND_USERNAME)" \
    "$(env_get FLEET_MANAGER_BITCOIND_PASSWORD)" getblockchaininfo)
  printf 'bitcoin core %s%s\n' \
    "$(jq -r '.result | "\(.chain), block \(.blocks), \(.verificationprogress * 100 | floor)% verified"' \
      <<<"${reply%$'\n'*}" 2>/dev/null || echo "RPC not answering")" \
    "$([[ -r $btc_marker ]] && echo ", version $(kv_get "$btc_marker" version), managed here")"
  find "$backups" -maxdepth 1 -name '*.tar' -printf 'backup       %p\n' 2>/dev/null || true
  journalctl -u "$service" -n 10 --no-pager || true
}

# Run by fleet-manager-update-check.timer: records the release, changes nothing.
cmd_check() {
  local cur pin status out=${STATE_DIRECTORY:-$prefix/var/lib/fleet-manager-setup}
  cur=$(installed)
  pin=$(production_pin)
  published "$pin" || die "the production pin $pin has no published $image image"
  if [[ -z $cur ]]; then
    status=not-installed
  elif [[ $pin == "$cur" ]]; then
    status=identical
  else
    status=$(compare "$cur" "$pin")
  fi
  mkdir -p "$out"
  printf '%s %s %s\n' "$pin" "$status" "$(date -u +%FT%TZ)" >"$out/.available.tmp"
  mv -f "$out/.available.tmp" "$out/available"
  case $status in
  ahead) echo "new Manifold Guardian release $pin (installed $cur); apply it with: fleet-manager-setup update" ;;
  behind | diverged) echo "the production pin $pin is $status relative to the installed $cur; not offered" ;;
  esac
}

case ${1-} in
"")
  cat <<EOF
Manifold Guardian, bare-metal install (advanced)
  You run Bitcoin Core, the firewall, backups and monitoring yourself; see
  bare-metal.md. Umbrel or StartOS is the simpler path.
1) Install  2) Update  3) Status  4) Update Bitcoin Core
EOF
  case $(ask "choice") in
  1) cmd_install ;;
  2) cmd_update ;;
  3) cmd_status ;;
  4) cmd_update_btc ;;
  esac
  ;;
install) cmd_install ;;
update)
  shift
  [[ ${1-} == --yes ]] && assume_yes=1 && shift
  cmd_update "$@"
  ;;
update-bitcoin-core)
  shift
  [[ ${1-} == --yes ]] && assume_yes=1 && shift
  cmd_update_btc "$@"
  ;;
status) cmd_status ;;
check) cmd_check ;;
*) die "usage: fleet-manager-setup [install | update [--yes] [<commit>] | update-bitcoin-core [--yes] [<version>] | status | check]" ;;
esac
