#!/usr/bin/env bash
# Exercises fleet-manager-setup.sh under a scratch prefix, with stub curl, nix,
# systemctl and tar on PATH, the way validate.sh stubs fleet-manager.
set -euo pipefail

cd "$(dirname "$0")/../../.."
repo_root=$PWD
script=$repo_root/packages/fleet-manager/systemd/fleet-manager-setup.sh
tmp=$(mktemp -d)
trap 'kill $(jobs -p) 2>/dev/null; rm -rf "$tmp"' EXIT
old=$(printf 'a%.0s' {1..40})
new=$(printf 'b%.0s' {1..40})
older=$(printf 'c%.0s' {1..40})
bin=$tmp/bin
mkdir -p "$bin"

cat >"$bin/curl" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$@" >>"$FAKE/curl.argv"
echo "$*" >>"$FAKE/curl.calls"
url=${*: -1}
case $url in
*docker-compose.yml) cat "$FAKE/compose" ;;
*ghcr.io/token*) echo '{"token":"t"}' ;;
*ghcr.io/v2/*/manifests/*)
  if grep -qx "${url##*/}" "$FAKE/published"; then echo 200; else echo 404; exit 22; fi ;;
*api.github.com/*/compare/*) printf '{"status":"%s"}\n' "$(cat "$FAKE/compare")" ;;
*api.github.com/repos/bitcoin/bitcoin/releases/latest) echo '{"tag_name":"v31.1"}' ;;
https://bitcoincore.org/bin/*)
  while [[ $1 != -o ]]; do shift; done
  cp "$FAKE/btcdl/${url##*/}" "$2" ;;
http://127.0.0.1:18443)
  # Read the credentials from the -K config like curl does; refuse one password.
  args=("$@")
  for i in "${!args[@]}"; do [[ ${args[i]} == -K ]] && cfg=${args[i + 1]}; done
  rpcpass=$(sed -n 's/^user = "[^:]*:\(.*\)"$/\1/p' "$cfg")
  if [[ -s $FAKE/refused-pass && $rpcpass == "$(cat "$FAKE/refused-pass")" ]]; then
    printf '\n401'
    exit 0
  fi
  case $* in
  *getnetworkinfo*) printf '{"result":{"subversion":"/Satoshi:%s.0/"}}' "$(cat "$FAKE/btc-running")" ;;
  *) printf '{"result":{"chain":"%s","blocks":0,"verificationprogress":1}}' "$(cat "$FAKE/chain")" ;;
  esac
  printf '\n%s' "$(cat "$FAKE/rpc-code")" ;;
http://127.0.0.1:*) ;;
*) echo "stub curl: unexpected $url" >&2; exit 2 ;;
esac
EOF
cat >"$bin/nix" <<'EOF'
#!/usr/bin/env bash
args=" $* "
if [[ $args == *" build "* ]]; then
  [[ -e $FAKE/build-fails ]] && exit 1
  while [[ $1 != --out-link ]]; do shift; done
  mkdir -p "$2/bin" && printf '#!/bin/sh\n' >"$2/bin/fleet-manager" && chmod +x "$2/bin/fleet-manager"
elif [[ $args == *" prefetch "* ]]; then
  last=${*: -1}
  printf '{"storePath":"%s","locked":{"rev":"%s"}}\n' "$REPO_ROOT" "${last##*/}"
fi
EOF
cat >"$bin/systemctl" <<'EOF'
#!/usr/bin/env bash
echo "$*" >>"$FAKE/systemctl.log"
case $1 in
--version) echo "systemd 255 (255.4)" ;;
is-active) [[ $(cat "$FAKE/active") == yes ]] ;;
daemon-reload) [[ ! -e $FAKE/reload-fails ]] ;;
cat) [[ -e $FLEET_MANAGER_SETUP_PREFIX/etc/systemd/system/$2 ]] ;;
show)
  # MainPID of a unit: the fake node's pid, unless a test names another.
  if [[ " $* " == *" MainPID "* ]]; then cat "$FAKE/mainpid" 2>/dev/null || head -n 1 "$FAKE/btc-pid"; else echo 0; fi ;;
stop) echo no >"$FAKE/active" ;;
start | restart) [[ -e $FAKE/start-fails ]] && echo crash-loop >"$FAKE/active" || echo yes >"$FAKE/active" ;;
esac
EOF
cat >"$bin/fail-tar" <<'EOF'
#!/usr/bin/env bash
# Dies mid-write, leaving a partial archive behind like a real tar would.
while [[ $# -gt 0 && $1 != -cf ]]; do shift; done
[[ $# -gt 1 ]] && echo partial >"$2"
exit 1
EOF
printf '#!/usr/bin/env bash\n' >"$bin/journalctl"
printf '#!/usr/bin/env bash\necho "$*" >>"$FAKE/useradd.log"\n' >"$bin/useradd"
# Who holds the RPC port: listener-pid, a process the test chose.
cat >"$bin/ss" <<'EOF'
#!/usr/bin/env bash
printf 'LISTEN 0 4096 127.0.0.1:18443 0.0.0.0:* users:(("x",pid=%s,fd=7))\n' "$(cat "$FAKE/listener-pid")"
EOF
# Bitcoin Core detection and the disk picker see only what these report.
printf '#!/usr/bin/env bash\n[[ -s $FAKE/btc-pid ]] && cat "$FAKE/btc-pid"\n' >"$bin/pgrep"
printf '#!/usr/bin/env bash\ncat "$FAKE/btc-unit"\n' >"$bin/ps"
cat >"$bin/findmnt" <<'EOF'
#!/usr/bin/env bash
case " $* " in
*" --target "*) echo / ;;
*" -t "*) cat "$FAKE/mounts" ;;
*) exit 1 ;;
esac
EOF
cat >"$bin/lsblk" <<'EOF'
#!/usr/bin/env bash
[[ $1 == -J* ]] && exec cat "$FAKE/lsblk.json"
# Device identity: the second look sees devid-later when a test sets it.
n=$(($(cat "$FAKE/lsblk-n" 2>/dev/null || echo 0) + 1))
echo "$n" >"$FAKE/lsblk-n"
if ((n > 1)) && [[ -e $FAKE/devid-later ]]; then cat "$FAKE/devid-later"; else echo "8:240 21474836480 SPARE1 0x5000"; fi
EOF
printf '#!/usr/bin/env bash\necho "$*" >>"$FAKE/mount.log"\n' >"$bin/mount"
printf '#!/usr/bin/env bash\n[[ ! -e $FAKE/mkfs-fails ]] || exit 1\necho "$*" >>"$FAKE/mkfs.log"\n' >"$bin/mkfs.ext4"
printf '#!/usr/bin/env bash\necho 0f0e-uuid\n' >"$bin/blkid"
printf '#!/usr/bin/env bash\n' >"$bin/wipefs"
cat >"$bin/git" <<'EOF'
#!/usr/bin/env bash
mkdir -p "${*: -1}/builder-keys" && touch "${*: -1}/builder-keys/builder.gpg"
EOF
cat >"$bin/gpg" <<'EOF'
#!/usr/bin/env bash
[[ " $* " == *" --verify "* ]] || exit 0
# One status line per signature; same-key makes every signature come from one key.
for i in $(seq "$(cat "$FAKE/goodsigs")"); do
  [[ -e $FAKE/same-key ]] && i=1
  echo "[GNUPG:] GOODSIG KEY$i builder$i"
done
EOF
# The host may already have a fleet-manager user; the installer must not depend on
# it. "id -u bitcoin" answers as if the node ran as the test's own user.
cat >"$bin/id" <<EOF
#!/usr/bin/env bash
[[ \$* == "-u bitcoin" ]] && exec $(command -v id) -u
exit 1
EOF
# The Nix build sandbox has no /usr/bin/env.
sed -i "1s|.*|#!$BASH|" "$bin"/*
chmod +x "$bin"/*

# The fake node used for detection: only its argv matters (pgrep is stubbed).
printf '#!%s\nwhile :; do sleep 1; done\n' "$BASH" >"$tmp/bitcoind"
chmod +x "$tmp/bitcoind"
# A real executable named bitcoind (a copy of bash; coreutils may be a multi-call
# binary that refuses another name) holds the RPC port.
mkdir -p "$tmp/node"
cp "$BASH" "$tmp/node/bitcoind"
"$tmp/node/bitcoind" -c 'while :; do sleep 1; done' &
listener_pid=$!

pass=Abc-123_def.XYZ
fake() { # fresh prefix with $old installed and running
  rm -rf "$tmp/p" "$tmp/f"
  mkdir -p "$tmp/f" "$tmp/p/var/lib/fleet-manager"
  touch "$tmp/f/systemctl.log" "$tmp/f/curl.argv"
  export FAKE=$tmp/f FLEET_MANAGER_SETUP_PREFIX=$tmp/p REPO_ROOT=$repo_root
  echo "image: ghcr.io/fedibtc/manifold-fman:$new" >"$FAKE/compose"
  printf '%s\n' "$old" "$new" >"$FAKE/published"
  echo ahead >"$FAKE/compare"
  echo regtest >"$FAKE/chain"
  echo yes >"$FAKE/active"
  echo state >"$tmp/p/var/lib/fleet-manager/fleet.sqlite"
  local r
  for r in "$old" "$older"; do
    mkdir -p "$tmp/p/opt/fleet-manager/releases/$r"
    touch "$tmp/p/opt/fleet-manager/releases/$r/.complete"
  done
  ln -s "releases/$old" "$tmp/p/opt/fleet-manager/current"
  mkdir -p "$tmp/p/etc/fleet-manager"
  echo FLEET_MANAGER_ADMIN_HTTP_BIND=127.0.0.1:8482 >"$tmp/p/etc/fleet-manager/fleet-manager.env"
  echo 200 >"$FAKE/rpc-code"
  echo 5 >"$FAKE/goodsigs"
  echo bitcoind.service >"$FAKE/btc-unit"
  echo "/ /dev/root ext4 999999999999" >"$FAKE/mounts"
  echo '{"blockdevices":[]}' >"$FAKE/lsblk.json"
  : >"$FAKE/btc-pid"
  echo "$listener_pid" >"$FAKE/listener-pid"
}
run() { # a run expected to succeed shows its output when it does not
  PATH="$bin:$PATH" FLEET_MANAGER_SETUP_SETTLE=0 "$script" "$@" >"$tmp/out" 2>&1 && return 0
  [[ -n ${expect_failure-} ]] || cat "$tmp/out" >&2
  return 1
}
fails() { # expected-message cmd...
  local msg=$1
  shift
  if expect_failure=1 run "$@"; then
    echo "expected '$*' to fail with '$msg'" >&2
    cat "$tmp/out" >&2
    exit 1
  fi
  grep -qF -- "$msg" "$tmp/out" || {
    echo "'$*' failed without '$msg':" >&2
    cat "$tmp/out" >&2
    exit 1
  }
}
current() { readlink "$tmp/p/opt/fleet-manager/current"; }
# `! cmd` is exempt from set -e, so negative checks go through this.
none() {
  if "$@"; then
    echo "unexpected success: $*" >&2
    exit 1
  fi
}
not_stopped() { none grep -q '^stop' "$FAKE/systemctl.log"; }

# check: records a newer published pin and changes nothing.
fake
run check
grep -q "^$new ahead " "$tmp/p/var/lib/fleet-manager-setup/available"
grep -qF "new Manifold Guardian release $new" "$tmp/out"
not_stopped

# The release oracle refuses anything but exactly one published pin.
fake
printf 'a: ghcr.io/fedibtc/manifold-fman:%s\nb: ghcr.io/fedibtc/manifold-fman:%s\n' "$new" "$older" >"$FAKE/compose"
fails "ambiguous production pin" check
fake
echo "image: ghcr.io/fedibtc/manifold-fman:latest" >"$FAKE/compose"
fails "no ghcr.io/fedibtc/manifold-fman:<commit>" check
fake
head -c 1000001 /dev/zero | tr '\0' x >"$FAKE/compose"
fails "larger than 1 MB" check
fake
echo "$old" >"$FAKE/published"
fails "has no published" check

# update refuses a pin that is not strictly newer, before stopping anything.
for status in behind diverged; do
  fake
  echo "$status" >"$FAKE/compare"
  fails "$new is $status relative to the installed $old" update --yes
  not_stopped
  [[ $(current) == "releases/$old" ]]
done
fake
echo "image: ghcr.io/fedibtc/manifold-fman:$old" >"$FAKE/compose"
run update --yes
grep -qF "Up to date: $old" "$tmp/out"
not_stopped

# update: stopped backup, switch, start, prune all but the old and new releases.
fake
run update --yes
[[ $(current) == "releases/$new" ]]
backup=$(echo "$tmp"/p/var/backups/fleet-manager/"$old"-*.tar)
[[ $(stat -c %a "$backup") == 600 ]]
tar -tf "$backup" | grep -qx 'fleet-manager/fleet.sqlite'
[[ -x $tmp/p/opt/fleet-manager/releases/$new/entrypoint.sh && -e $tmp/p/opt/fleet-manager/releases/$old ]]
[[ $(head -n 1 "$tmp/p/opt/fleet-manager/releases/$new/entrypoint.sh") == "#!$tmp/p/opt/fleet-manager/releases/$new/bash/bin/bash" ]]
cmp <(tail -n +2 "$tmp/p/opt/fleet-manager/releases/$new/entrypoint.sh") <(tail -n +2 "$repo_root/packages/fleet-manager/entrypoint.sh")
[[ ! -e $tmp/p/opt/fleet-manager/releases/$older ]]
grep -qx 'phase=done' "$tmp/p/opt/fleet-manager/update.state"
[[ $(cat "$FAKE/active") == yes ]]

# A failure before the switch restarts the untouched old release.
fake
ln -s "$bin/fail-tar" "$bin/tar"
fails "update aborted before the switch; $old is unchanged and running again" update --yes
rm "$bin/tar"
[[ $(current) == "releases/$old" && $(cat "$FAKE/active") == yes ]]
partials=("$tmp"/p/var/backups/fleet-manager/*.partial)
[[ ! -e ${partials[0]} ]]
grep -qx 'phase=aborted' "$tmp/p/opt/fleet-manager/update.state"

# So does a failure after the switch but before the new daemon starts.
fake
touch "$FAKE/reload-fails"
fails "update aborted before the switch; $old is unchanged and running again" update --yes
[[ $(current) == "releases/$old" && $(cat "$FAKE/active") == yes ]]

# A run that died after stopping an already stopped guardian does not start it.
fake
echo no >"$FAKE/active"
printf 'phase=stopping\nfrom=%s\nto=%s\nwas_active=no\n' "$old" "$new" >"$tmp/p/opt/fleet-manager/update.state"
ln -s "$bin/fail-tar" "$bin/tar"
fails "update aborted before the switch; $old is unchanged" update --yes
rm "$bin/tar"
not_started() { none grep -qx 'start fleet-manager.service' "$FAKE/systemctl.log"; }
not_started

# A new release that does not stay up is stopped, not rolled back.
fake
touch "$FAKE/start-fails"
fails "$new did not stay up and is stopped" update --yes
[[ $(current) == "releases/$new" && $(cat "$FAKE/active") == no ]]
grep -qx 'phase=failed' "$tmp/p/opt/fleet-manager/update.state"
# A retry that still fails stops it again instead of leaving it crash-looping.
fails "$new did not stay up and is stopped" update --yes "$new"
[[ $(cat "$FAKE/active") == no ]]
grep -qx 'phase=failed' "$tmp/p/opt/fleet-manager/update.state"
# Re-running update retries the start once the cause is fixed.
rm "$FAKE/start-fails"
run update --yes "$new"
grep -qx 'phase=done' "$tmp/p/opt/fleet-manager/update.state"

# An unverified start is verified on re-run even if the service looks active.
fake
rm "$tmp/p/opt/fleet-manager/current"
ln -s "releases/$new" "$tmp/p/opt/fleet-manager/current"
mkdir -p "$tmp/p/opt/fleet-manager/releases/$new"
printf 'phase=starting\nfrom=%s\nto=%s\n' "$old" "$new" >"$tmp/p/opt/fleet-manager/update.state"
run update --yes "$new"
none grep -qF "Up to date" "$tmp/out"
grep -qx 'phase=done' "$tmp/p/opt/fleet-manager/update.state"

# --------------------------------------------------------------- install
# A running "bitcoind" for detection; only its argv matters (pgrep is stubbed).
start_fake_btc() {
  rm -rf "$tmp/btcdata"
  mkdir -p "$tmp/btcdata"
  printf 'server=1\nrpcauth=manifold:oldsalt$oldhash\n[test]\nrpcport=18332\n  rpcauth=manifold:indented$x\n' \
    >"$tmp/btcdata/node.conf"
  "$tmp/bitcoind" -datadir="$tmp/btcdata" -conf=node.conf &
  echo $! >"$FAKE/btc-pid"
}
fresh() { # nothing installed yet
  fake
  rm -rf "$tmp/p/opt/fleet-manager" "$tmp/p/etc/fleet-manager" "$tmp/p/var/lib/fleet-manager"
}
env_file=$tmp/p/etc/fleet-manager/fleet-manager.env
env_pass() { sed -n 's/^FLEET_MANAGER_BITCOIND_PASSWORD=//p' "$env_file"; }
# Checked with Python's hmac, as Bitcoin Core's share/rpcauth/rpcauth.py computes it.
rpcauth_matches() { # conf user password
  python3 - "$@" <<'EOF'
import hmac, sys
conf, user, password = sys.argv[1:]
line = [l for l in open(conf) if l.startswith(f"rpcauth={user}:")][0].strip()
salt, digest = line.split(":", 1)[1].split("$")
sys.exit(hmac.new(salt.encode(), password.encode(), "sha256").hexdigest() != digest)
EOF
}
make_btc_release() { # version → a signed-looking release in $FAKE/btcdl
  local a=bitcoin-$1-$(uname -m)-linux-gnu.tar.gz
  mkdir -p "$tmp/rel/bitcoin-$1/bin" "$FAKE/btcdl"
  printf '#!/bin/sh\n' >"$tmp/rel/bitcoin-$1/bin/bitcoind"
  chmod +x "$tmp/rel/bitcoin-$1/bin/bitcoind"
  tar -czf "$FAKE/btcdl/$a" -C "$tmp/rel" "bitcoin-$1"
  (cd "$FAKE/btcdl" && sha256sum "$a" >SHA256SUMS)
  echo signatures >"$FAKE/btcdl/SHA256SUMS.asc"
}
managed_btc() { # version: a Bitcoin Core this installer manages, as after an install
  mkdir -p "$tmp/p/opt/bitcoin-core/bitcoin-$1/bin"
  ln -sfn "bitcoin-$1" "$tmp/p/opt/bitcoin-core/current"
  printf 'version=%s\ndatadir=%s\nconf=%s\n' "$1" "$tmp/p/var/lib/bitcoind" "$tmp/p/etc/bitcoin/bitcoin.conf" \
    >"$tmp/p/etc/fleet-manager/bitcoin-core.managed"
  printf 'FLEET_MANAGER_BITCOIND_URL=http://127.0.0.1:18443\nFLEET_MANAGER_BITCOIND_USERNAME=manifold\nFLEET_MANAGER_BITCOIND_PASSWORD=%s\n' \
    "$pass" >>"$env_file"
}

# An existing node and the operator's own credentials: the node is left alone.
fresh
start_fake_btc
printf 'development\n\nhttp://127.0.0.1:18443\nn\nmanifold\nbad"pw\n' >"$tmp/answers"
fails "RPC password is not accepted" install <"$tmp/answers"
[[ ! -e $env_file ]]
printf 'development\n\nhttp://127.0.0.1:18443\nn\nmanifold\n%s\n\n\n\n' "$pass" >"$tmp/answers"
run install <"$tmp/answers"
[[ $(stat -c %a "$env_file") == 600 && $(stat -c %a "$tmp/p/etc/fleet-manager/admin-password") == 400 ]]
grep -qx "FLEET_MANAGER_BITCOIND_PASSWORD=$pass" "$env_file"
grep -qx 'FLEET_MANAGER_MANIFOLD_ENVIRONMENT=development' "$env_file"
none grep -qF "$pass" "$FAKE/curl.argv"
[[ $(current) == "releases/$new" && -e $tmp/p/etc/systemd/system/fleet-manager.service ]]
grep -q -- '--system' "$FAKE/useradd.log"
grep -q 'oldsalt' "$tmp/btcdata/node.conf"
none grep -qs fleet-manager "$tmp/p/etc/fstab"
# Re-running keeps the credentials; a node on the wrong chain is refused.
echo main >"$FAKE/chain"
printf 'development\n\nhttp://127.0.0.1:18443\n\n' >"$tmp/answers"
fails "the development environment needs 'regtest'" install <"$tmp/answers"

# An existing node gets a generated rpcauth user above its first [section].
fresh
start_fake_btc
printf 'development\n\nhttp://127.0.0.1:18443\ny\n\n\n\n' >"$tmp/rpcauth-answers"
run install <"$tmp/rpcauth-answers"
conf=$tmp/btcdata/node.conf
[[ $(grep -c '^rpcauth=manifold:' "$conf") == 1 ]]
none grep -q oldsalt "$conf"
none grep -q indented "$conf"
# Every request to the RPC port bypasses any proxy.
awk '/127\.0\.0\.1:18443/ { n++; if (!/--noproxy/) bad++ } END { exit !(n > 0 && !bad) }' "$FAKE/curl.calls"
awk '/^rpcauth=manifold:/ { a = NR } /^\[test\]/ { b = NR } END { exit !(a && b && a < b) }' "$conf"
grep -qx 'rpcport=18332' "$conf"
backups_=("$conf".bak-*)
grep -q oldsalt "${backups_[0]}"
rpcauth_matches "$conf" manifold "$(env_pass)"
grep -qx 'restart bitcoind.service' "$FAKE/systemctl.log"
# ...but a node outside systemd is never edited or restarted.
fresh
start_fake_btc
echo session-2.scope >"$FAKE/btc-unit"
cp "$conf" "$tmp/conf.orig"
fails "is not the main process of a systemd service" install <"$tmp/rpcauth-answers"
cmp "$conf" "$tmp/conf.orig"

# No node: install a Bitcoin Core of our own, verified.
printf 'development\n\ny\n\n\n\n\n\n' >"$tmp/btc-answers"
fresh
make_btc_release 31.1
echo 31.1 >"$FAKE/btc-running"
run install <"$tmp/btc-answers"
btc_conf=$tmp/p/etc/bitcoin/bitcoin.conf
grep -qx chain=regtest "$btc_conf"
grep -qx server=1 "$btc_conf"
none grep -q '^prune=' "$btc_conf"
[[ $(stat -c %a "$btc_conf") == 640 ]]
rpcauth_matches "$btc_conf" manifold "$(env_pass)"
[[ $(readlink "$tmp/p/opt/bitcoin-core/current") == bitcoin-31.1 ]]
grep -qF "ExecStart=$tmp/p/opt/bitcoin-core/current/bin/bitcoind -conf=$btc_conf -datadir=$tmp/p/var/lib/bitcoind " \
  "$tmp/p/etc/systemd/system/bitcoind.service"
grep -qx version=31.1 "$tmp/p/etc/fleet-manager/bitcoin-core.managed"
grep -qx 'enable --now bitcoind.service' "$FAKE/systemctl.log"
run status
grep -qF 'regtest, block 0, 100% verified, version 31.1, managed here' "$tmp/out"
# A release without 3 builder signatures, or not matching them, installs nothing.
fresh
make_btc_release 31.1
echo 2 >"$FAKE/goodsigs"
fails "only 2 distinct builders signed" install <"$tmp/btc-answers"
[[ ! -e $tmp/p/opt/bitcoin-core/bitcoin-31.1 ]]
# Five signatures from one key are one builder.
fresh
make_btc_release 31.1
touch "$FAKE/same-key"
fails "only 1 distinct builders signed" install <"$tmp/btc-answers"
[[ ! -e $tmp/p/opt/bitcoin-core/bitcoin-31.1 ]]
fresh
make_btc_release 31.1
echo tampered >>"$FAKE/btcdl/bitcoin-31.1-$(uname -m)-linux-gnu.tar.gz"
fails "does not match its signed SHA256SUMS" install <"$tmp/btc-answers"
[[ ! -e $tmp/p/opt/bitcoin-core/bitcoin-31.1 ]]

# update-bitcoin-core: only a node installed here, only forward, only when it reports the version.
fake
fails "was not installed by fleet-manager-setup" update-bitcoin-core --yes
managed_btc 30.2
make_btc_release 31.1
echo 31.1 >"$FAKE/btc-running"
run update-bitcoin-core --yes
[[ $(readlink "$tmp/p/opt/bitcoin-core/current") == bitcoin-31.1 ]]
grep -qx version=31.1 "$tmp/p/etc/fleet-manager/bitcoin-core.managed"
grep -qx 'restart bitcoind.service' "$FAKE/systemctl.log"
[[ -d $tmp/p/opt/bitcoin-core/bitcoin-30.2 ]]
run update-bitcoin-core --yes
grep -qF "Bitcoin Core is up to date: 31.1" "$tmp/out"
fails "older than the installed 31.1" update-bitcoin-core --yes 30.2
fake
managed_btc 30.2
make_btc_release 31.1
echo 30.2 >"$FAKE/btc-running"
fails "does not report version 31.1" update-bitcoin-core --yes
grep -qx version=30.2 "$tmp/p/etc/fleet-manager/bitcoin-core.managed"

# Disks: data on another mounted filesystem is bind-mounted at /var/lib/fleet-manager.
fresh
start_fake_btc
printf '/ /dev/root ext4 999999999999\n%s /dev/sdb1 ext4 999999999999\n' "$tmp/p/mnt/big" >"$FAKE/mounts"
printf 'development\n\nhttp://127.0.0.1:18443\nn\nmanifold\n%s\n\n2\n\n' "$pass" >"$tmp/answers"
run install <"$tmp/answers"
grep -qxF "$tmp/p/mnt/big/fleet-manager $tmp/p/var/lib/fleet-manager none bind,nofail,x-systemd.requires-mounts-for=$tmp/p/mnt/big 0 0" \
  "$tmp/p/etc/fstab"
[[ -d $tmp/p/mnt/big/fleet-manager ]]
grep -qF "$tmp/p/var/lib/fleet-manager" "$FAKE/mount.log"
# An unmounted disk is formatted only after its device path is typed.
fresh
start_fake_btc
echo '{"blockdevices":[{"name":"/dev/sdz","size":21474836480,"type":"disk","fstype":null,"mountpoint":null,"model":"Spare"}]}' \
  >"$FAKE/lsblk.json"
printf 'development\n\nhttp://127.0.0.1:18443\nn\nmanifold\n%s\n\n2\n/dev/sdy\n' "$pass" >"$tmp/answers"
fails "not confirmed; /dev/sdz is untouched and nothing changed" install <"$tmp/answers"
[[ ! -s $FAKE/mkfs.log && ! -e $env_file ]]
printf 'development\n\nhttp://127.0.0.1:18443\nn\nmanifold\n%s\n\n2\n/dev/sdz\n\n' "$pass" >"$tmp/answers"
run install <"$tmp/answers"
grep -q -- /dev/sdz "$FAKE/mkfs.log"
grep -qxF "UUID=0f0e-uuid $tmp/p/mnt/sdz ext4 defaults,noatime,nofail,x-systemd.device-timeout=15 0 2" "$tmp/p/etc/fstab"
grep -qF "$tmp/p/mnt/sdz/fleet-manager $tmp/p/var/lib/fleet-manager none bind" "$tmp/p/etc/fstab"

# A symlinked config is never written through.
fresh
start_fake_btc
mv "$tmp/btcdata/node.conf" "$tmp/elsewhere.conf"
ln -s "$tmp/elsewhere.conf" "$tmp/btcdata/node.conf"
cp "$tmp/elsewhere.conf" "$tmp/conf.orig"
fails "is not a regular file; add the rpcauth line yourself" install <"$tmp/rpcauth-answers"
cmp "$tmp/elsewhere.conf" "$tmp/conf.orig"

# Another local process holding the RPC port (say during a restart) never gets
# the credentials.
fresh
make_btc_release 31.1
echo 31.1 >"$FAKE/btc-running"
sleep 600 &
echo $! >"$FAKE/listener-pid"
fails "is held by a process that is not Bitcoin Core; the credentials were not sent" install <"$tmp/btc-answers"
none grep -qx 'http://127.0.0.1:18443' "$FAKE/curl.argv"
kill "$(cat "$FAKE/listener-pid")"

# An update killed right after stopping brings the old release back first, even
# when the network (the pin lookup) then fails.
fake
echo no >"$FAKE/active"
printf 'phase=stopping\nfrom=%s\nto=%s\nbackup=%s\nwas_active=yes\n' "$old" "$new" "$tmp/p/b.tar" \
  >"$tmp/p/opt/fleet-manager/update.state"
mkdir -p "$tmp/p/var/backups/fleet-manager" && echo partial >"$tmp/p/b.tar.partial"
rm "$FAKE/compose"
fails "cannot fetch the production pin" update --yes
[[ $(cat "$FAKE/active") == yes && ! -e $tmp/p/b.tar.partial ]]
grep -qx phase=aborted "$tmp/p/opt/fleet-manager/update.state"

# Production uses a Bitcoin Core on this host, as production-releases.md requires.
fresh
start_fake_btc
printf 'production\n\nhttps://node.example:8332\n' >"$tmp/answers"
fails "production needs Bitcoin Core on this host" install <"$tmp/answers"

# A symlink planted where the data root goes on the chosen disk is refused.
fresh
start_fake_btc
printf '/ /dev/root ext4 999999999999\n%s /dev/sdb1 ext4 999999999999\n' "$tmp/p/mnt/big" >"$FAKE/mounts"
mkdir -p "$tmp/p/mnt/big" "$tmp/elsewhere"
ln -s "$tmp/elsewhere" "$tmp/p/mnt/big/fleet-manager"
printf 'development\n\nhttp://127.0.0.1:18443\nn\nmanifold\n%s\n\n2\n\n' "$pass" >"$tmp/answers"
fails "fleet-manager is a symlink; refusing to use it" install <"$tmp/answers"
none grep -qs "$tmp/p/mnt/big/fleet-manager" "$tmp/p/etc/fstab"

# A bitcoind that merely sits inside a system service (a user's cron job is in
# cron.service) is not that service's main process: it is never trusted.
fresh
start_fake_btc
echo cron.service >"$FAKE/btc-unit"
echo 1 >"$FAKE/mainpid"
printf 'development\n\nhttp://127.0.0.1:18443\nn\nmanifold\n%s\n\n\n\n' "$pass" >"$tmp/answers"
fails "is held by a process that is not Bitcoin Core; the credentials were not sent" install <"$tmp/answers"

# A process that only calls itself bitcoind (a script named so) is not trusted.
fresh
make_btc_release 31.1
echo 31.1 >"$FAKE/btc-running"
"$tmp/bitcoind" -impostor &
echo $! >"$FAKE/listener-pid"
fails "is held by a process that is not Bitcoin Core; the credentials were not sent" install <"$tmp/btc-answers"
kill "$(cat "$FAKE/listener-pid")"

# Remote RPC only over https: the credentials travel with every request.
fresh
start_fake_btc
printf 'development\n\nhttp://10.0.0.5:8332\n' >"$tmp/answers"
fails "would send the credentials unencrypted" install <"$tmp/answers"

# A node with credentials on its command line, or several nodes: never edited.
fresh
"$tmp/bitcoind" -datadir="$tmp/btcdata" -conf=node.conf -rpcauth=other:salt\$hash &
echo $! >"$FAKE/btc-pid"
cp "$tmp/btcdata/node.conf" "$tmp/conf.orig"
printf 'development\n\nhttp://127.0.0.1:18443\nmanifold\n%s\n\n\n\n' "$pass" >"$tmp/answers"
run install <"$tmp/answers"
grep -qF "takes RPC credentials on its command line" "$tmp/out"
cmp "$tmp/btcdata/node.conf" "$tmp/conf.orig"
fresh
start_fake_btc
cat "$FAKE/btc-pid" "$FAKE/btc-pid" >"$tmp/two" && mv "$tmp/two" "$FAKE/btc-pid"
run install <"$tmp/answers"
grep -qF "2 bitcoind processes are running" "$tmp/out"
cmp "$tmp/btcdata/node.conf" "$tmp/conf.orig"

# Saved credentials that Bitcoin Core now refuses are replaced, not kept.
fresh
start_fake_btc
printf 'development\n\nhttp://127.0.0.1:18443\nn\nmanifold\n%s\n\n\n\n' "$pass" >"$tmp/answers"
run install <"$tmp/answers"
echo "$pass" >"$FAKE/refused-pass"
printf 'development\n\nhttp://127.0.0.1:18443\ny\n\n' >"$tmp/answers"
run install <"$tmp/answers"
grep -qF "refuses the saved RPC user manifold" "$tmp/out"
[[ $(env_pass) != "$pass" ]]
rpcauth_matches "$tmp/btcdata/node.conf" manifold "$(env_pass)"

# A managed node whose credentials were never saved (interrupted install) is re-keyed.
fresh
mkdir -p "$tmp/p/etc/fleet-manager" "$tmp/p/etc/bitcoin"
printf 'chain=regtest\nserver=1\nrpcauth=manifold:lost$hash\n' >"$tmp/p/etc/bitcoin/bitcoin.conf"
managed_btc 31.1
sed -i '/BITCOIND_PASSWORD/d' "$env_file"
printf 'development\n\n\n\n\n' >"$tmp/answers"
run install <"$tmp/answers"
none grep -q lost "$tmp/p/etc/bitcoin/bitcoin.conf"
rpcauth_matches "$tmp/p/etc/bitcoin/bitcoin.conf" manifold "$(env_pass)"
grep -qx 'restart bitcoind.service' "$FAKE/systemctl.log"

# A disk that changed between the question and formatting is not formatted.
fresh
start_fake_btc
echo '{"blockdevices":[{"name":"/dev/sdz","size":21474836480,"type":"disk","fstype":null,"mountpoint":null,"model":"Spare"}]}' \
  >"$FAKE/lsblk.json"
echo "8:250 21474836480 OTHER 0x6000" >"$FAKE/devid-later"
printf 'development\n\nhttp://127.0.0.1:18443\nn\nmanifold\n%s\n\n2\n/dev/sdz\n\n' "$pass" >"$tmp/answers"
fails "/dev/sdz changed since you chose it; nothing was formatted" install <"$tmp/answers"
[[ ! -s $FAKE/mkfs.log ]]
# A failing mkfs stops the install: no fstab entry, no data placed on the wrong disk.
fresh
start_fake_btc
echo '{"blockdevices":[{"name":"/dev/sdz","size":21474836480,"type":"disk","fstype":null,"mountpoint":null,"model":"Spare"}]}' \
  >"$FAKE/lsblk.json"
touch "$FAKE/mkfs-fails"
expect_failure=1 run install <"$tmp/answers" && { echo "install with a failing mkfs succeeded" >&2; exit 1; }
none grep -qs sdz "$tmp/p/etc/fstab"
[[ ! -e $tmp/p/mnt/sdz/fleet-manager && ! -e $env_file.placed ]]

# fstab without a final newline keeps its last entry intact.
fresh
start_fake_btc
printf '/ /dev/root ext4 999999999999\n%s /dev/sdb1 ext4 999999999999\n' "$tmp/p/mnt/big" >"$FAKE/mounts"
mkdir -p "$tmp/p/etc"
printf 'UUID=root-uuid / ext4 defaults 0 1' >"$tmp/p/etc/fstab"
printf 'development\n\nhttp://127.0.0.1:18443\nn\nmanifold\n%s\n\n2\n\n' "$pass" >"$tmp/answers"
run install <"$tmp/answers"
grep -qx 'UUID=root-uuid / ext4 defaults 0 1' "$tmp/p/etc/fstab"
grep -qxF "$tmp/p/mnt/big/fleet-manager $tmp/p/var/lib/fleet-manager none bind,nofail,x-systemd.requires-mounts-for=$tmp/p/mnt/big 0 0" \
  "$tmp/p/etc/fstab"

# The dashboard password is a plain owned 0400 file: LoadCredential='s ACL reads as
# group bits, which the daemon refuses (seen on systemd 259).
unit=$repo_root/packages/fleet-manager/systemd/fleet-manager.service
none grep -q '^LoadCredential' "$unit"
grep -qx 'Environment=FLEET_MANAGER_ADMIN_HTTP_PASSWORD_FILE=/etc/fleet-manager/admin-password' "$unit"

echo "fleet-manager-setup: all checks passed"
