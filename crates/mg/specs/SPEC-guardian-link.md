# SPEC-guardian-link: the operator's Fedi app linked to an MG

## Record justification

The link is a contract between three code bases: the MG daemon
(`mg-core::guardian_link`, wire types in
`crates/service-manifold-guardian/src/guardian_link.rs`), the Fedi push gateway
([SPEC-hook-invocation](../../push-gateway/specs/SPEC-hook-invocation.md)),
the Fedi app (repository `fedibtc/fedi`), which creates the hook, parses the
link URI, and shows the host's web UI, and the platform packages, which set
the host-UI address. No local artifact can own what the app and the daemon
must agree on.

## Contract

An MG operator links one Fedi app installation to their MG so the daemon
can push a notification when the fleet needs the operator's attention, and so
the app can open the host's own web UI through the MG. The
daemon never contacts the phone itself: it invokes a push-gateway hook the
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
  `fedi/fman/guardian-link/1`, the `node` of the URI; Iroh authenticates that
  endpoint. The app takes the `fman` key from the URI as given, to name the
  guardian: nothing binds it to `node`, because the URI comes from the host's
  own dashboard and a wrong name can only mislabel the link. `link_device`
  is the app's only call on this ALPN. It carries no device key: the MG
  records the Iroh endpoint id the call arrived from, which Iroh's handshake
  authenticates, as the linked device's
  ([The host's web UI](#the-hosts-web-ui)). The app therefore dials from a
  stable endpoint key of its own for this MG, derived from its wallet seed
  and this MG's `node`, so the id names one installation's wallet to one MG
  and links nothing across MGs. One device is linked at a time:
  `link_device` replaces the previous device and its endpoint.
- **Hook.** `link_device` carries a hook URL under the Manifold
  environment's push-gateway origin, else `PushGatewayUnusable`. The app creates
  the hook with its own fixed title and body, workflow `guardian_link`, and
  no TTL, so it never expires. The daemon's invocation carries no
  `data`: nothing about the host's seats, federations, amounts, messages, or
  reasons leaves it. The delivered push still holds the gateway's own `pg.*`
  fields, taken from the hook the app created; the app recognizes the push by
  `pg.workflow`, not by an empty payload. The gateway's production cap is six
  accepted invocations per hook per hour
  ([SPEC-abuse-limits](../../push-gateway/specs/SPEC-abuse-limits.md)), and
  that is the cadence this design accepts: every notification is one generic "look at your dashboard". The
  app shows no guardian status of its own; the dashboard is where the host
  sees why.
- **Reasons.** One per failing check of the latest readiness report, the
  checks the dashboard's Health page shows: `relay_check_failed`,
  `discovery_check_failed`, `bitcoin_check_failed`, `version_check_failed`
  (below the policy's `min_mg_version`), and `uptime_check_failed`. Then
  `update_recommended`, when this release is below the feature
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
  readiness report, a new feature policy, or a write to the support
  thread; and when the link is made or revoked. It has no scan timer; the
  only timed wake is the retry of a failed send.
- **Delivery states.** A transient failure is retried with the completion
  callback backoff. A definitive rejection (hook not found, expired or
  revoked, policy rejected) parks the link as `terminal` with the reason and
  clears the hook; the dashboard shows that the phone must be linked again,
  and the daemon sends nothing until it is. Max-uses and rate limiting are
  transient: the next tick after the window sends again. A send the gateway
  holds because the phone is inactive (`503 hook_target_inactive`: the provider
  rejected its token and the app has not re-registered) marks the link
  `inactive`; it is retried like any transient failure, the dashboard asks the
  host to open the Fedi app on the phone, and the next delivered notification
  makes the link `active` again. An outcome for a
  replaced phone's hook never touches the new link: every update is guarded
  on the link's `linked_at_ms`.
- **Lifetime.** A link has no end date. A gateway registration does not expire
  by age, only when the provider rejects its token
  ([SPEC-hook-invocation](../../push-gateway/specs/SPEC-hook-invocation.md)).
  The app re-registers whenever FCM issues a new token, and on start when its
  token differs from the one it last registered, so a rotation the provider made
  while the app was not running is caught when it next opens. A rejected
  registration is held for 30 days (the link is `inactive` meanwhile);
  re-registering in that time resumes delivery with no relink. The link ends
  when the host unlinks, when the app links to another MG, or 30 days after the
  provider rejected the phone's token without the app coming back, as after a
  reinstall. The gateway then answers the next invocation for good, the link
  parks as `terminal`, and the dashboard offers to reconnect the phone with a
  new code.
- **Unlink.** Only the host ends the link, with `RevokeGuardianLink` on the
  dashboard; the daemon then holds no device and no hook. The app has no
  unlink of its own; when it links to another MG it revokes its previous
  hook at the gateway.

## The host's web UI

The linked device may open the web UI of the host the MG runs on (the
umbrelOS or StartOS dashboard), to update the MG app or anything else the host
offers there, from anywhere, over the same Iroh endpoint.

- **Target.** The daemon is started with one host-UI address
  (`--host-ui-addr`), set by the platform package to where that platform
  serves its dashboard as seen from the MG's container: `10.21.0.1:80` on
  umbrelOS (the app network's gateway) and `10.0.3.1:80` on StartOS (the
  container bridge). The caller never names a target. Without the option there
  is nothing to forward.
- **Wire.** ALPN `fedi/fman/guardian-link-host-ui/1`. Each bidirectional
  stream is one new TCP connection to the host-UI address, and bytes pass
  through unchanged both ways until each side finishes. The MG parses nothing:
  the app speaks HTTP/1.1 on the stream, and a WebSocket upgrade continues on
  it. The host's own login is the only authentication of what is said there;
  the MG adds and strips nothing.
- **Gate.** The MG serves a connection only from the endpoint id recorded at
  the current link, and only while the guardian link is
  [on](#enabling-mobile-notifications). It closes any other connection with an
  application code before serving a stream: `1` not linked (also when nothing
  is linked, or the link predates recorded endpoints, so a stranger learns
  nothing else), `2` the guardian link is off, `3` no host-UI address. A
  connection already open is closed the same way when a relink, a revoke, or
  the policy makes it fail the gate.
- **Bounds.** At most 32 streams are open at once across all connections,
  which covers a browser's connections to one origin plus its WebSockets; a
  stream beyond that is reset with code `2`. A stream whose TCP connection
  fails, or is not accepted within five seconds, is reset with code `1`. The
  MG does not bound bytes per stream: it does not parse requests, the only
  caller is the operator's own linked device, and the host's UI is the
  operator's anyway.

## Enabling mobile notifications

The feature policy
([SPEC-setup-payment-federations](../../../specs/SPEC-setup-payment-federations.md))
carries an optional `guardian_link` flag. The guardian link is on only while the
latest policy the MG admitted sets it to `true`, or, without an admitted
policy or a flag in it, its Manifold environment's default is on.
Development and staging default to on and production to off
([SPEC-manifold-environment](../../manifold-environment/specs/SPEC-manifold-environment.md)),
so test deployments need no publication and production waits for Fedi to
publish `true`. A published `false` turns it off in any environment.

While it is off, `GuardianLink` reports `availability: "disabled"` and the
dashboard shows neither the Mobile notifications page nor its navigation item, so a
host does not learn the feature exists. `CreateGuardianLinkOffer` and
`TestGuardianLinkNotification` fail, `link_device` answers `InvalidInvite`
even for an offer made while it was on, the host-UI forward refuses the
linked device, and the notifier sends nothing and records nothing as told. A link stored while it was on is kept; when the flag
turns it on again, the next notifier pass tells the reasons current then.
`RevokeGuardianLink` works either way. When on, `availability` is
`no_push_gateway` in an environment without a push-gateway origin
(development, unless a local gateway is named), else
`ready`.

The link is one row (`guardian_link`) in the daemon's database, with the
linked device's endpoint id; the open offer
is memory only and does not survive a restart. The admin verbs `GuardianLink`,
`CreateGuardianLinkOffer`, `RevokeGuardianLink`, and
`TestGuardianLinkNotification` are the dashboard's whole surface
([SPEC-admin-socket](./SPEC-admin-socket.md)).
