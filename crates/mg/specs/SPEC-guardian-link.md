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
  the gateway's longest TTL, one year. The daemon's invocation carries no
  `data`: nothing about the host's seats, federations, amounts, messages, or
  reasons leaves it. The delivered push still holds the gateway's own `pg.*`
  fields, taken from the hook the app created; the app recognizes the push by
  `pg.workflow`, not by an empty payload. The gateway's production cap is two
  accepted invocations per hook per hour, and that is the cadence this design
  accepts: every notification is one generic "look at your dashboard". The
  app shows no guardian status of its own; the dashboard is where the host
  sees why.
- **Reasons.** `seat_failed` (a formed seat lost its data), `seat_unavailable`
  (a formed seat's child has been down for ten minutes),
  `not_ready_for_new_seats` (the latest readiness report fails),
  `payment_federation_not_receivable` (a plan is offered and an accepted
  payment federation cannot receive), `not_approved` (the directory shows no
  Holder authorization), and `support_message` (unread Fedi support message).
  The operator's test notification carries no reason. A seat still waiting on
  its FI's DKG is idle, not a reason.
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
- **Expiry.** Nothing renews the hook. When it expires, or the app is
  reinstalled, the gateway rejects the next invocation for good and the link
  parks as `terminal`; the dashboard then shows a fresh code to link the phone
  again. A host relinks about once a year in exchange for the phone holding no
  key and making no calls after linking.
- **Unlink.** Only the host ends the link, with `RevokeGuardianLink` on the
  dashboard; the daemon then holds no device and no hook. The app has no
  unlink of its own; when it links to another MG it revokes its previous
  hook at the gateway.

The link is one row (`guardian_link`) in the daemon's database; the open offer
is memory only and does not survive a restart. The admin verbs `GuardianLink`,
`CreateGuardianLinkOffer`, `RevokeGuardianLink`, and
`TestGuardianLinkNotification` are the dashboard's whole surface
([SPEC-admin-socket](./SPEC-admin-socket.md)).
