# Setup-payment publisher

This production-only tool signs and publishes the complete kind-37707 policy
defined by [`SPEC-setup-payment-federations`](../../specs/SPEC-setup-payment-federations.md).
Copy `example-policy.json` and replace every placeholder; the committed template
is deliberately invalid so it cannot be published unchanged.

Obtain `--expected-publisher` independently, out of band from the secret-key
source. Build the reproducible tool with `nix build .#setup-payment-publisher`.
The secret is never accepted through argv or an environment variable.

For the first publication, after confirming out of band that no earlier receipt
or publication exists:

```console
password-manager read tsp-publisher-key | \
  nix run .#setup-payment-publisher -- publish \
    --content policy.json \
    --expected-publisher <hex-or-npub> \
    --first-publication \
    --receipt signed-event.json
```

For every update, supply the latest retained receipt:

```console
password-manager read tsp-publisher-key | \
  nix run .#setup-payment-publisher -- publish \
    --content policy.json \
    --expected-publisher <hex-or-npub> \
    --previous-receipt previous-signed-event.json \
    --receipt new-signed-event.json
```

The custodian must retain the newest receipt as the publisher high-water mark.
The tool checks it against every Production relay before loading the secret.
Publishing an empty stop-set additionally requires `--allow-empty-stop-set`.
Exactly one publish or republish operation for this key may run at a time across
all custodians and hosts; the custody procedure must serialize operations and
retain the winning newest receipt. Relay preflight is not a distributed lock.

If one relay fails after the receipt is saved, retry the same signed event
without the secret:

```console
nix run .#setup-payment-publisher -- republish \
  --receipt signed-event.json \
  --expected-publisher <hex-or-npub>
```

To let MGs start direct-proof guardian transfers, set
`"guardian_transfer_v2": true` in the policy file; set `false` to stop new
transfers. If you omit the flag, each environment's default applies: on in
development and staging, off in production. MGs continue transfers they
already stored regardless of the flag.

To turn on the MG's mobile notifications (guardian link), set `"guardian_link": true` in the policy
file; set `false` to hide it again. If you omit the flag, each environment's
default applies: on in development and staging, off in production.

The retired `guardian_transfer` field must never be set to `true` in production.
MG 0.1.9 and older FA transfer readers use it to enable the incompatible old
`TransferSeat` shape; they ignore the new flag and stay off in production
when the retired field is absent. This publisher no longer accepts the
retired field.

Before enabling the new flag in production, upgrade both source and destination
MGs and set `min_mg_version` to the first release with the new shape. MGs
that understand this minimum stop advertising new seats when below it;
older readers may still advertise, so endpoint upgrades remain required.
The minimum version is not a protocol
handshake and does not recover existing mixed-version transfers. To recover
those transfers, upgrade both endpoints. See
[Enabling transfer](../mg/specs/SPEC-guardian-transfer.md#enabling-transfer).

To stop MGs below a release from advertising new seats, for versions that
understand the minimum, set
`"min_mg_version": "0.1.9"` (a SemVer release no newer than `fman_version`).
Those MGs keep running their seats and show that an update is needed; their
uptime is not affected. Omitted, there is no minimum.

MGs from 0.1.16 on stop taking new seats while their seats keep waiting for
CPU, disk or memory. Omitted, `max_pressure_percent` means
`{"cpu": {"6h": 15, "3.5d": 10}, "io": {"6h": 25, "3.5d": 15}, "memory": {"6h": 70, "3.5d": 70}}`: each value is
the highest percent of wall time (1 to 100) during which a task of the MG's
cgroup may have stalled on that resource (Linux PSI `some`), as a moving
average with a 6-hour or a 3.5-day time constant. Set only the windows to
change; the rest keep these values. An MG over any maximum, or that cannot
read its pressure, stops advertising and quoting new seats. It keeps running
its seats, and its uptime is not affected. MGs journal their averages and
maximums, so tune the values from the seat readiness telemetry.

MGs from 0.1.16 on measure their CPU capacity once, as a score of the MG's
CPU benchmark in iterations per second summed over all CPUs, and journal it
as `cpu_iterations_per_sec` (with `cpu_count` and
`cpu_single_core_iterations_per_sec`) in the `seat capacity` telemetry. To
limit seats by CPU, set `"seat_cpu_iterations_per_sec": 40` (from 1 to
100,000): each MG then sells no more than its score divided by that budget,
rounded down. Omitted, there is no CPU limit, and nothing defaults it; set it
from the telemetry.

To ask operators below a release to update without stopping their sales, set
`"recommended_min_mg_version": "0.2.0"` (also no newer than `fman_version`).
An MG below it sends its linked phone one notification. Omitted, nothing is
recommended.

MGs whose operators set a monthly data budget sell only the seats that
budget covers, at the network data one seat uses. To change that figure from
its default of 65, set for example `"seat_monthly_data_gb": 60`, in GB (10^9
bytes) per 30 days, from 1 to 10,000. Derive it from the fleet's
`fm_peer_transport_bytes_total`, transmitted plus received.

MGs from 0.1.17 on also sell no more than their data filesystem's size
divided by the disk one seat needs, which stands in for drive wear. To change
that figure from its default of 80, set for example `"seat_disk_gb": 100`, in
GB (10^9 bytes), from 1 to 100,000. Tune it from the fleet's
`cgroup_block_write_bytes` and `data_filesystem_bytes` in the `cgroup
resource sample`.

To roll a feature out to a share of seats, set `"rollout"` to the schedule
of each feature by name, for example
`"rollout": {"aleph_backup_batch": [[1795000000, 0], [1795086400, 10]]}`:
each point is a Unix time in seconds and the percentage of seats (0 to 100)
that run the feature then. Between points the percentage moves linearly,
before the first point it is the first point's and after the last point the
last point's. Each seat selects itself with a private random draw, so a
higher percentage adds seats and a lower one removes them; a publication
cannot pick seats. MGs that know no feature by that name ignore it.

Every change of a schedule must start from the percentage the previous
publication gives the feature at the new event's timestamp, which the tool
checks to within one point; a new feature starts at 0. To pause, publish one
point at the current percentage; to speed up, slow down or roll back, start a
new ramp from it, for example `[[now, 40], [now + 1800, 0]]` to remove the
feature over half an hour. MGs that lose their relays keep following the
last schedule they hold, so publish ramps of at most about a day and extend
them with later publications. `--allow-rollout-jump` acknowledges a larger
step, which flips that share of seats at once.

The JSON file is the shared wire object, not a tool-specific schema. Rebuild
after protocol changes; the golden serialization test forces additions or
serialized defaults to be reflected in operator policy files. Older consumers
that admit unknown fields ignore the new flag; only the initial release
rejects fields it does not know.

## Testing

Pure tests use fake relay operations and never contact Production. The ignored
real-relay test uses an exclusive local `defe` lease; see [`testing.md`](./testing.md).
