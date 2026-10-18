# Manifold Guardian on bare-metal Linux (systemd + Nix)

> **Advanced install.** This path assumes you administer a Linux server
> yourself: Nix, systemd, Bitcoin Core configuration, the firewall, backups and
> monitoring are your responsibility, and nothing here does them for you. If
> that is not you, use the Umbrel or StartOS package instead
> ([README](./README.md)): they handle Bitcoin Core, updates and backups for you.

For operators who run a Linux server rather than Umbrel or StartOS.
`fleet-manager-setup` builds the same `fleet-manager` the published image
carries, from the same commit, and runs it as a hardened systemd service. It
has a menu with four actions:

- **Install**: finds your Bitcoin Core (or installs one), lets you choose the
  disk for the data, builds the production release, and starts the guardian.
- **Update**: installs a newer production release, after a backup of the
  stopped data root.
- **Status**: what is installed, what is available, the service and Bitcoin
  Core state, and the recent log.
- **Update Bitcoin Core**: for a Bitcoin Core the installer installed.

A daily timer looks up the current production release and records it. It
never changes the host; updates happen only when the operator runs Update.

This is an operator installer, not a qualified platform release. Releases are
qualified as described in [production-releases.md](./production-releases.md)
(including `scripts/test-mg-upgrade.sh` for data upgrades), and the
[secure-deployment checklist](./secure-deployment.md) still applies in full;
see [Operator obligations](#operator-obligations).

## Requirements

- Linux with systemd 247 or later.
- [Nix](https://nixos.org/download), multi-user (daemon) install.
- Bitcoin Core on the network of the chosen environment (mainnet for
  `production`), installed by the installer or already running as the main
  process of a systemd service. The installer sends RPC credentials only to a
  root-owned `bitcoind` that is a system service's main process.
- Disk for the Nix store and the data root, plus room for one backup of the
  data root per update. The guardian limits its seats to what the host's
  available memory can carry (about 1.2 GiB each,
  [REQ-seat-capacity-default](../../crates/mg/specs/REQ-seat-capacity-default.md)).

## Build time

Every Install and Update builds `fleet-manager` for its commit. The installer
adds the fedimint binary cache, but the guardian's own Rust code is compiled
on the host. On a small machine (few cores, little RAM) this can take
many hours, and it is easy to mistake for a hang. Plan for it:

- Run the installer inside `tmux` or `screen`: a dropped SSH session kills the
  build. During Update the guardian keeps running while the new release builds;
  it stops only for the backup and the switch.
- On a low-memory host, limit build parallelism in `/etc/nix/nix.conf`
  (`max-jobs = 1`, `cores = 2`) and add swap, so the compiler is not killed.
- Or build somewhere faster. A store path depends only on the commit and the
  platform, so a build on any machine with the same platform (for example
  `x86_64-linux`) is the one the installer would make. Copy it over before you
  run Install or Update, and the installer finds it already in the store:

  ```sh
  # on the fast machine
  out=$(nix build --no-link --print-out-paths github:fedibtc/manifold/<commit>#fleet-manager)
  nix copy --to ssh://root@<guardian-host> "$out"
  ```

## Bitcoin Core

The guardian authenticates to Bitcoin Core with its own `rpcauth` user, never
the cookie: it reads its credentials once at start and hands them to every
seat, so a cookie that rotates when Bitcoin Core restarts would leave the seats
with dead credentials. Install looks for a running `bitcoind` and offers what
fits:

- **A node is running.** Install shows its config file and either adds a
  generated `rpcauth=manifold:…` line to it (above the first `[section]`, so
  it applies to every network; a dated backup of the file stays beside it) and
  restarts the node through its systemd service, or takes credentials you
  created yourself (`share/rpcauth/rpcauth.py` in the Bitcoin Core source).
  Adding the user restarts your node; a node that does not run under systemd
  is never edited or restarted.
- **No node.** Install can install Bitcoin Core: it downloads the release from
  bitcoincore.org, accepts it only if at least 3 Bitcoin Core builders (from
  `bitcoin-core/guix.sigs`) signed its `SHA256SUMS` and the archive matches
  that file, and runs it as its own `bitcoin` user and `bitcoind.service`, on
  the chain of the chosen environment, with a generated `rpcauth` user. On
  mainnet you choose a full node (about 800 GB) or one pruned to 10 GB; a
  pruned node needs the Esplora fallback for the blocks it no longer has. The
  first sync takes days, and seats work only once it is done.

RPC stays on loopback, which is Bitcoin Core's default. `production` needs
Bitcoin Core on the same host ([production-releases.md](./production-releases.md)),
so there the installer accepts only a `127.0.0.1` URL; in staging or
development a node on another host needs an `https://` URL, since the
credentials travel with every request. Before it sends credentials to a local
port, the installer checks that the port is held by `bitcoind` running as the
node's user, not by another process waiting for a restart. The guardian assumes
a single-tenant host (no untrusted local users), as its own trust model does. Credentials Bitcoin
Core starts refusing are noticed on the next Install and replaced.

## Disks

Install asks where the guardian's data (and a new Bitcoin Core's blockchain)
should live, and lists:

- mounted filesystems, with their free space. One that a non-root user owns or
  can write to (or any directory above it) is refused, since root creates and
  chowns directories there;
- unmounted disks. One with an ext4, xfs or btrfs filesystem is mounted as is.
  Any other is **formatted ext4, erasing it**, and only after you type its
  device path. The disk is added to `/etc/fstab` by UUID with `nofail`.

Data on another filesystem is kept in `<mount>/fleet-manager` and bind-mounted
at `/var/lib/fleet-manager`. The service requires that mount, so if the disk
is missing it fails instead of starting on an empty directory, where it would
look like a new guardian. The Nix store stays where Nix put it (`/nix`).

## Install

Pick the installer commit you want to run (master, or a commit you reviewed)
and run it as root:

```sh
sudo /nix/var/nix/profiles/default/bin/nix --extra-experimental-features 'nix-command flakes' \
  run --accept-flake-config github:fedibtc/manifold/<commit>#fleet-manager-setup
```

Choose **Install**. It checks the RPC credentials against Bitcoin Core (the
chain must match the environment), offers the current production release, and
then sets up:

| Path | What |
| --- | --- |
| `/opt/fleet-manager/releases/<commit>` | `fleet-manager` (a Nix out-link, so a GC root) and that commit's `entrypoint.sh` |
| `/opt/fleet-manager/current` | the running release |
| `/opt/fleet-manager/setup`, `/usr/local/sbin/fleet-manager-setup` | the installer, for the timer and later runs |
| `/etc/fleet-manager/fleet-manager.env` | environment, RPC URL and credentials, dashboard bind (root, 0600) |
| `/etc/fleet-manager/admin-password` | dashboard password (`fleet-manager`, 0400; the directory is 0750 `root:fleet-manager`) |
| `/var/lib/fleet-manager` | the data root (`fleet-manager` user, 0700) |
| `/var/backups/fleet-manager` | stopped data-root backups, one per update (root, 0600) |
| `fleet-manager.service`, `fleet-manager-update-check.timer` | in `/etc/systemd/system` |
| `/opt/bitcoin-core/bitcoin-<version>`, `/opt/bitcoin-core/current` | a Bitcoin Core installed here, and the running version |
| `/etc/bitcoin/bitcoin.conf`, `bitcoind.service` | its config (root:bitcoin, 0640) and service |
| `/etc/fleet-manager/bitcoin-core.managed` | marks that Bitcoin Core as managed by the installer |

Install asks every question first and changes nothing on the host (apart from
its lock file in `/run`) until the last answer. Re-running it is safe: it keeps the installed release, the data
location, the dashboard password and working Bitcoin Core credentials, and
rewrites the environment file from your answers.

Open UDP 30000-30031 inbound for the seats' iroh sockets (see
[Seat iroh reachability](./README.md#seat-iroh-reachability)). The dashboard
binds `127.0.0.1:8482` by default; reach it with
`ssh -L 8482:127.0.0.1:8482 <host>`, or bind a LAN or tailnet address at
install time. Never publish it to the internet. Then open the dashboard and
follow the onboarding. Write down the recovery words it shows: they are the
only backup of the guardian's identity.

## Updates

`fleet-manager-update-check.timer` runs daily. It reads the production release
from the Umbrel app's image pin (the project has no release tags), confirms
that the image was published to GHCR, and asks GitHub whether that commit is
ahead of the installed one. The result appears in **Status**, and the journal
of `fleet-manager-update-check.service` has a line when a newer release exists.

`sudo fleet-manager-setup update` (or **Update** in the menu) then:

1. refuses a candidate that is not published, or is behind or diverged from the
   installed commit;
2. builds the new release while the guardian keeps running;
3. stops the guardian, writes a tar of the stopped data root to
   `/var/backups/fleet-manager/<old commit>-<time>.tar` and checks that it reads back;
4. switches `current`, starts the new release, and requires it to stay up for
   30 seconds and the dashboard to answer;
5. keeps the previous release and removes older ones.

If anything fails before step 4, the old release is restarted and nothing has
changed. If the new release fails to start, the guardian is left stopped:
the new daemon may already have migrated the data root, and in-place downgrade
is not supported. Fix the cause and re-run Update, which retries the start, or
restore the backup.

## Updating Bitcoin Core

For a Bitcoin Core that Install installed, **Update Bitcoin Core** in the menu
(or `sudo fleet-manager-setup update-bitcoin-core`) offers the latest release,
verifies it the same way, switches `/opt/bitcoin-core/current`, restarts
`bitcoind.service`, and checks that the node answers and reports the new
version. The guardian keeps running: its `rpcauth` credentials do not change.
Downgrades are refused, and the previous version stays in `/opt/bitcoin-core`.
A Bitcoin Core you installed yourself is yours to update.

## Restore a backup

Restore the whole data root, never part of it, together with the release that
wrote it:

```sh
sudo systemctl stop fleet-manager
# Unpack beside the data root, then swap the contents: this works whether
# /var/lib/fleet-manager is a plain directory or a bind mount from another disk.
sudo mkdir /var/lib/fleet-manager.restore /var/lib/fleet-manager.broken
sudo tar --numeric-owner -xpf /var/backups/fleet-manager/<old commit>-<time>.tar -C /var/lib/fleet-manager.restore
# A supported restore excludes the safe-event journals (SPEC-guardian-telemetry-proxy).
sudo find /var/lib/fleet-manager.restore -type d -name safe-events -prune -exec rm -rf {} +
sudo find /var/lib/fleet-manager -mindepth 1 -maxdepth 1 -exec mv -t /var/lib/fleet-manager.broken {} +
sudo find /var/lib/fleet-manager.restore/fleet-manager -mindepth 1 -maxdepth 1 -exec mv -t /var/lib/fleet-manager {} +
sudo ln -sfn releases/<old commit> /opt/fleet-manager/.current.tmp
sudo mv -fT /opt/fleet-manager/.current.tmp /opt/fleet-manager/current
sudo systemctl start fleet-manager
```

Keep `/var/lib/fleet-manager.broken` until the restored guardian is healthy.

If `releases/<old commit>` was removed, rebuild it first, with the same
`nix build … --out-link` and `entrypoint.sh` copy the installer uses.

## Operating

- Logs: `journalctl -fu fleet-manager`.
- CLI: `sudo -u fleet-manager /opt/fleet-manager/current/fleet-manager/bin/fman-cli --data-dir /var/lib/fleet-manager <command>`.
- Changing RPC credentials or the dashboard bind: re-run Install.
- Updates run the installer you have, with the new release's daemon and
  entrypoint. Before updating across a release that changes the unit or its
  environment, run the newer installer (`nix run …/<new commit>#fleet-manager-setup`).
- The RPC password reaches the daemon as a command-line argument, which the
  daemon's single-tenant trust model accepts. On a host with other local users,
  mount `/proc` with `hidepid=invisible` so they cannot read it from `ps`.

## Operator obligations

The unit runs one guardian per host on one data root, as its own user, with
the data root, the secrets and the admin socket closed to everyone else. It
also pins an exact commit. The rest of the
[secure-deployment checklist](./secure-deployment.md) is yours:

- monitoring of disk space, the process, seats, relay publication and wallets;
- a tested backup and restore, including the recovery words;
- abuse controls in front of public FA RPC;
- keeping the dashboard behind the network boundary you chose.
