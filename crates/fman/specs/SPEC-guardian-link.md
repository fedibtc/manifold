# SPEC-guardian-link: the operator's Fedi app linked to an FMan

## Record justification

The link is a contract between three code bases: the FMan daemon
(`fman-core::guardian_link`, wire types in
`crates/service-fleet-manager/src/guardian_link.rs`), the Fedi push gateway
([SPEC-hook-invocation](../../push-gateway/specs/SPEC-hook-invocation.md)),
and the Fedi app (repository `fedibtc/fedi`), which creates the hook and
parses the link URI. No local artifact can own what the app and the daemon
must agree on.

## Contract

An FMan operator links one Fedi app installation to their FMan so the daemon
can push a notification when the fleet needs the operator's attention. The
daemon never talks to the phone directly: it invokes a push-gateway hook the
app created, exactly as it invokes an FI's DKG completion hook.

- **Link offer.** The dashboard (`CreateGuardianLinkOffer`) mints a 32-byte
  secret the daemon keeps in memory for ten minutes and shows as the URI
  `fedi://guardian-link?v=1&fman=<nostr hex>&node=<iroh endpoint id>&env=<manifold environment>&secret=<hex>`,
  rendered as a QR code. A new offer replaces the open one. The secret is
  consumed by the first successful `link_device`; a wrong, used, or expired
  secret is one coarse `InvalidInvite`. The operator may link only through
  the dashboard; nothing on the FI RPC surface creates an offer.
- **Device identity.** The app signs later verbs with a per-installation
  BIP-340 key (`device_id`), using the FI signed-request envelope
  ([SPEC-signed-envelopes](../../service-fleet-manager/specs/SPEC-signed-envelopes.md))
  with labels `guardian_link/get_attention`, `guardian_link/renew_callback`,
  and `guardian_link/unlink_device`. The daemon accepts a signed verb only
  from the linked device; any other signer, a bad signature, or a stale
  timestamp is `Unauthorized`. One device is linked at a time: `link_device`
  replaces the previous device.
- **Transport.** The app dials the FMan's Iroh endpoint with ALPN
  `fedi/fman/guardian-link/1`, the `node` of the URI, and checks that the
  `fman` key is the one the dashboard showed.
- **Hook.** `link_device` and `renew_callback` carry a hook URL under the
  deployment's configured push-gateway origin, else `PushGatewayUnusable`.
  The app creates the hook with its own fixed title and body and
  `pg.workflow = guardian_link`; the daemon's invocation `data` carries only
  `reasons`, a comma-separated list of reason codes. No seat id, federation
  id, amount, or message text leaves the host. The gateway's production cap
  is two accepted invocations per hook per hour, and that is the cadence
  this design accepts: every notification is one generic "look at your
  FMan" and the app fetches the reasons with `get_attention`.
- **Reasons.** `seat_failed` (a formed seat lost its data), `seat_unavailable`
  (a formed seat's child has been down for ten minutes),
  `not_ready_for_new_seats` (the latest readiness report fails),
  `payment_federation_not_receivable` (a plan is offered and an accepted
  payment federation cannot receive), `not_approved` (the directory shows no
  Holder authorization), `support_message` (unread Fedi support message), and
  `test` (the operator's test). A seat still waiting on its FI's DKG is idle,
  not a reason.
- **Edge triggering.** The daemon notifies when a reason is present that the
  device has not been told about, sending every current reason in one
  invocation. It remembers the reasons it told; a reason that clears is
  forgotten so its return is notified again. A reason that stays set is not
  repeated. Each invocation's idempotency key is
  `guardian-link:<linked_at_ms>:<sequence>`, so a retry after a transient
  failure deduplicates at the gateway.
- **Delivery states.** A transient failure is retried with the completion
  callback backoff. A definitive rejection (hook not found, expired or
  revoked, policy rejected) parks the link as `terminal` with the reason and
  clears the hook; the dashboard shows that the phone must be linked again,
  and the daemon sends nothing until it is. Max-uses and rate limiting are
  transient: the next tick after the window sends again.
- **Renewal.** A hook has a bounded TTL (the gateway's maximum is a year; the
  app uses 30 days). The app renews before expiry with `renew_callback`,
  which replaces the hook and its expiry and reactivates a parked link.
- **Unlink.** Either side ends the link: the app with `unlink_device`, the
  operator with `RevokeGuardianLink`. The daemon then holds no device and no
  hook.

The link is one row (`guardian_link`) in the daemon's database; the open offer
is memory only and does not survive a restart. The admin verbs `GuardianLink`,
`CreateGuardianLinkOffer`, `RevokeGuardianLink`, and
`TestGuardianLinkNotification` are the dashboard's whole surface
([SPEC-admin-socket](./SPEC-admin-socket.md)).
