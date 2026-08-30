# Guardian telemetry: product decision and MVP implementation contract

Status: FMan exposure, authenticated admission, direct safe-journal collection,
and sparse timestamp-preserving metrics collection are implemented.
Product/legal release wording remains open, 2026-08-19.

For the implemented cloud collector's operator contract, start with
[`cloud-collector-deployment.md`](./cloud-collector-deployment.md). It lists
the environment controls production readiness assumes; it does not certify a
particular deployment.

## Decision (2026-07-22)

After stakeholder input, we will proceed with the **FMan-based approach** (Approach B).

Rationale:
- Marginally simpler to build — engineering estimate ~0.8x the FI-based effort; the difference is small enough that product and business reasons decide it.
- Consent folds cleanly into the Fedi-verified guardian terms (e.g. "when using Fedi-verified guardians, you consent to privacy-preserving telemetry"), so the FI is never shown an obtrusive consent checkbox.
- No meaningful loss versus FI-based: telemetry is scoped to Fedi-verified federations anyway, and FI-based's only unique reach was non-Fedi-verified federations — so choosing FMan-based gives up nothing in scope.

One point to honor in the terms: even the FMan-based flow records data *about* the federation, so the terms should subtly make the FI/guardians aware that this telemetry is recorded, kept non-obtrusive.

## Context

Decentralized federations run guardians that Fedi does not host. Today Fedi
gets per-federation telemetry directly from the guardians it hosts; once
guardians move to third-party Fleet Managers, that channel disappears. This
document lays out what telemetry we want, the two candidate ways to get it,
what has to be built regardless of which we pick, and the trade-offs of each.
It is intentionally non-technical: the goal is to align on an approach before
writing the detailed spec.

## What we want

Two private operational telemetry channels:

1. **Prometheus endpoint access** — gives vetted guardian metrics, including
   active/new wallet proxies from backup-count metrics.
2. **Safe-event journals** — gives typed events explicitly marked
   `safe_to_share = true`, separately from ordinary logs.

## Common to both approaches: a receiving service

Whichever approach we choose, Fedi needs a service that receives
**telemetry access info** — either from the FI (FI-based approach) or
from the FMan (FMan-based approach). For the MVP this service is minimal and
simple. This work has to happen regardless of the approach chosen.

## Approach A — FI-based

The prometheus endpoint is gathered from the FMan and passed to Fedi through
the FI. The FI relays the telemetry access info to us.

Open questions:

- Is FI **consent** involved in relaying this to us?
- Is it only for **Fedi-verified** setups, or for **all** setups?

If the FI consents, the FI can send everything we want — invite code +
prometheus endpoint + Fedi-verification proof — together in one payload. In that case we do **not** need to rely on being the gateway provider to
obtain the invite code.

### Invite-code visibility without FI consent

If we implement FI consent and do not get it, and we are the gateway
provider, we can still obtain the invite code. For federations where we do
**not** provide gateway service:

- if it is a publicly announced federation, we still get the invite code;
- if it is a hidden federation, we have no visibility.

## Approach B — FMan-based

For Fedi-verified FMans, we already publish a list of trusted federations for
setup payment. We could embed in that note a URL that FMans are expected to
use to push invite codes + prometheus endpoint data.

- This bypasses FIs altogether.
- It becomes an obligation of availing the PeerBadge verification service.
- Blind spot: for **non**-Fedi-verified FMans, we have no visibility.

Considerations:

- The FI is completely bypassed.
- The FMan is expected to provide this info as an obligation of being
  Fedi-verified.

## Open questions for stakeholders

- FI-based or FMan-based?

## Technical resolution (2026-08-05)

The FMan-based decision means authenticated pull **access**, not periodic
metric-value uploads. A verified FMan registers one Iroh locator and one
FMan-wide capability with the configured Fedi receiver. That capability covers
seat discovery, every Running seat's policy-projected `fedimintd` Prometheus
response, and the FMan and retained-seat safe-event journals. FMan applies the
compiled source allowlist before transport; it does not aggregate, snapshot,
archive, or publicly expose those channels.

The signed trusted-federation/setup-payment publication carries only the
receiver registration URL. It does not need to enumerate every FMan Iroh id:
each FMan supplies its current locator and FMan-wide capability in its authenticated
registration. This preserves endpoint rotation without republishing global
policy and works on home deployments without public HTTPS ingress.

The detailed transport and capability rationale is
[SPEC-guardian-telemetry-proxy](../../crates/fman/specs/SPEC-guardian-telemetry-proxy.md).
The enduring cloud collection boundary is
[ARCH-cloud-fman-telemetry](../../specs/ARCH-cloud-fman-telemetry.md).
Focused durability and wire-test coverage is documented in
[cloud-collector-testing.md](./cloud-collector-testing.md).

## Current implementation boundary

The collector durably owns verified targets, polls typed safe-event journals
every five minutes by default, and polls vetted metrics every 30 minutes by
default (15 minutes is the only other supported cadence). It preserves each bounded exact JSONL batch as an independently
compressed frame under `logs/<opaque-stream>/<UTC-day>.jsonl.zst`, with its
cursor and frame boundary committed together. It retains only the latest
successful metrics observation per seat and exposes that state on its private
`/metrics` listener with the original observation timestamp:

1. **Trusted-federation note** — the signed kind-37707 setup-payment schema
   carries one deployment-owned HTTPS `telemetry_registration_url`; it never
   publishes FMan locators or capabilities.
2. **FMan registration + sanitizing proxy** — each FMan periodically sends one
   idempotent FMan-level registration and serves capability-authenticated,
   default-deny projected metrics and safe-journal pull APIs over a dedicated Iroh
   ALPN. There is no per-seat capability, acknowledgement, generation, or
   receiver state. The owner-only local telemetry re-enrollment command rotates
   the one FMan-wide capability and wakes registration.
3. **Fedi receiving services** — the transitional push-gateway adapter verifies
   and encrypts each registered target and exposes a protected seat/metrics pull
   adapter. The standalone cloud collector owns direct polling and durable
   safe-journal archive and latest-metrics persistence. The push-gateway adapter does not deliver
   invites to Observer, expose journal HTTP routes, schedule pulls, preserve
   cursors, archive journals, or retain metrics.
4. **Consent / terms wording** — still open with product/legal. The implemented
   interim boundary conforms to current PeerBadge/Guardianito onboarding terms;
   it must be revisited when authoritative wording is supplied.
5. **Data scope** — the release baseline is
   [`metrics-privacy-inventory.md`](./metrics-privacy-inventory.md). Re-run it
   against every Fedimint/module pin. Compatible patch releases share one
   supported SemVer requirement, but no version range broadens the exact
   family/label policy. FMan applies that default-deny policy before transport
   and the collector repeats it as defense in depth.

Prometheus or Agent must scrape the private listener with:

```yaml
honor_timestamps: true
track_timestamps_staleness: true
```

Prometheus owns metrics history and remote-write state. Changing the supported
source requirement, method-source gate, or inventory revision atomically clears
the incompatible latest snapshots and poll deadlines before serving. A current
source-hash change alone remains diagnostic.
6. **Non-Fedi-verified FMans** — explicitly out of scope (blind spot
   acknowledged), since telemetry rides on the verification relationship.
