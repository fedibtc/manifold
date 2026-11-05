# SPEC-guardian-link: the operator's Fedi app linked to an MG

## Record justification

The link is a contract between three code bases: the MG daemon
(`mg-core::guardian_link`, wire types in
`crates/service-manifold-guardian/src/guardian_link.rs`), the Fedi push gateway
([SPEC-hook-invocation](../../push-gateway/specs/SPEC-hook-invocation.md)),
and the Fedi app (repository `fedibtc/fedi`), which creates the hook and
parses the link URI. No local artifact can own what the app and the daemon
must agree on.

## Contract

An MG operator links one Fedi app installation to their MG so the daemon
can push a notification when the fleet needs the operator's attention. The
daemon never talks to the phone directly: it invokes a push-gateway hook the
app created, exactly as it invokes an FI's DKG completion hook.

- **Link offer.** The dashboard (`CreateGuardianLinkOffer`) mints a 32-byte
  secret the daemon keeps in memory for ten minutes and shows as the URI
  `manifold:guardian-link?v=1&fman=<nostr hex>&node=<iroh endpoint id>&env=<manifold environment>&secret=<hex>`,
  rendered as a QR code. The scheme is Manifold's own, so the code names no
  particular app. A new offer replaces the open one. The secret is
  consumed by the first successful `link_device`; a wrong, used, or expired
  secret is one coarse `InvalidInvite`. The operator may link only through
  the dashboard; nothing on the FI RPC surface creates an offer.
- **Transport.** The app dials the MG's Iroh endpoint with ALPN
  `fedi/fman/guardian-link/1`, the `node` of the URI, and checks that the
  `fman` key is the one the dashboard showed. `link_device` is the app's only
  call; it carries no device key, since nothing after it needs one. One
  device is linked at a time: `link_device` replaces the previous device.
- **Hook.** `link_device` carries a hook URL under the deployment's
  configured push-gateway origin, else `PushGatewayUnusable`. The app creates
  the hook with its own fixed title and body, workflow `guardian_link`, and
  no TTL, so it never expires. The daemon's invocation carries no
  `data`: nothing about the host's seats, federations, amounts, messages, or
  reasons leaves it. The delivered push still holds the gateway's own `pg.*`
  fields, taken from the hook the app created; the app recognizes the push by
  `pg.workflow`, not by an empty payload. The gateway's production cap is two
  accepted invocations per hook per hour, and that is the cadence this design
  accepts: every notification is one generic "look at your dashboard". The
  app shows no guardian status of its own; the dashboard is where the host
  sees why.
- **Reasons.** One per failing check of the latest readiness report, the
  checks the dashboard's Health page shows: `relay_check_failed`,
  `discovery_check_failed`, `bitcoin_check_failed`, `version_check_failed`
  (below the policy's `min_mg_version`), and `uptime_check_failed`. Then
  `update_recommended`, when this release is below the setup-payment
  policy's `recommended_min_mg_version`
  ([SPEC-setup-payment-federations](../../../specs/SPEC-setup-payment-federations.md)),
  and `support_message`, an unread Fedi support message. Before the first
  readiness run no check counts as failing. The operator's test notification
  carries no reason.

- **Edge triggering.** The daemon notifies when a reason is present that the
  device has not been told about, sending one invocation for all current
  reasons. It remembers the reasons it told; a reason that clears is
  forgotten so its return is notified again. A reason that stays set is not
  repeated, so a recommended update is told once until this MG reaches it.
  A source not read yet since the daemon started (no readiness report or no
  policy yet) is unknown, not cleared, so a restart does not tell its reasons
  again.
  Each invocation's idempotency key is
  `guardian-link:<linked_at_ms>:<sequence>`, so a retry after a transient
  failure deduplicates at the gateway.
- **Wakes.** The notifier runs when a source of a reason changes: a new
  readiness report, a new setup-payment policy, or a write to the support
  thread; and when the link is made or revoked. It has no scan timer; the
  only timed wake is the retry of a failed send.
- **Delivery states.** A transient failure is retried with the completion
  callback backoff. A definitive rejection (hook not found, expired or
  revoked, policy rejected) parks the link as `terminal` with the reason and
  clears the hook; the dashboard shows that the phone must be linked again,
  and the daemon sends nothing until it is. Max-uses and rate limiting are
  transient: the next tick after the window sends again. An outcome for a
  replaced phone's hook never touches the new link: every update is guarded
  on the link's `linked_at_ms`.
- **Lifetime.** A link has no end date, and the app makes no call after linking:
  the gateway keeps a registration fresh while a hook with no expiry targets it
  ([SPEC-hook-invocation](../../push-gateway/specs/SPEC-hook-invocation.md)). The
  link ends when the host unlinks, when the app links to another MG, or when the
  provider rejects the phone's token. After a rejection, as after a reinstall
  once the old token has failed, the gateway answers the next invocation for good
  and the link parks as `terminal`. The dashboard then offers to reconnect the
  phone with a new code.
- **Unlink.** Only the host ends the link, with `RevokeGuardianLink` on the
  dashboard; the daemon then holds no device and no hook. The app has no
  unlink of its own; when it links to another MG it revokes its previous
  hook at the gateway.

The link is one row (`guardian_link`) in the daemon's database; the open offer
is memory only and does not survive a restart. The admin verbs `GuardianLink`,
`CreateGuardianLinkOffer`, `RevokeGuardianLink`, and
`TestGuardianLinkNotification` are the dashboard's whole surface
([SPEC-admin-socket](./SPEC-admin-socket.md)).
