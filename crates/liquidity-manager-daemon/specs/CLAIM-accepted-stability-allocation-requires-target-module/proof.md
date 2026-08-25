# Current argument

## Argument

**L1 (`code`) — acceptance checks no source-module capability.**
`VerificationPipeline::run_pipeline` authenticates the invite's federation id,
network, FMan seats, metadata, advertisements, credentials, and policy. Its
`FederationPreview` exposes federation id/config hash/network/seats/metadata,
but neither it nor `public::plan_allocation` reads the authenticated
client config's module kinds. The source-specific acceptance criterion is only
that the requested source is enabled in FLIP's Admin configuration; all
ordinary verification and capacity gates still apply. A nonzero stability
minimum creates a `StabilityPool` item.

**L2 (`code`) — funding begins before the module is accessed.** The stability
worker joins the target client, allocates a normal Fedimint wallet peg-in
address, persists it, creates its provider-wallet operation, and sends the
item's reserved amount. Only after the withdrawal is completed and that peg-in
reports `Claimed` does it call `target_wallet_balance` and eventually
`submit_deposit_to_provide`.

**L3 (`code`) — the first stability-module lookup is post-send and fails.**
After the claimed peg-in and primary-balance check,
`submit_deposit_to_provide` is the first stability-pool module lookup. It calls
`client.get_first_module::<StabilityPoolClientModule>()?`, which errors when
that module is absent. `advance_stability_deposit` clears its local `submitting`
marker and returns `unavailable`; it has no terminal recovery, target-client
sweep, or source downgrade. The item remains active while the provider's
claimed e-cash is in the FLIP-owned target client.

**L4 (`concrete execution`) — an endorsed non-stability federation reaches the
outflow.** Let FLIP's trusted Admin enable `SourceType::StabilityPool`. Form an
ordinary wallet/mint federation with endorsed FMan seats but no stability-pool
module (A1), then request only a positive stability minimum. L1 accepts it.
L2 sends the reserved provider amount and the target wallet claims it. L3 then
fails every deposit attempt because the client has no stability module. By A2,
the provider outflow occurred despite no target capability and no official
recovery path. This falsifies the claim.

## Residual windows

- The FI does not receive the e-cash directly: it is held by FLIP's target
  client. The impact is provider-fund lockup and capacity exhaustion, not an FI
  key compromise.
- A federation with a module of the right kind but hostile/economically
  unsuitable parameters is outside this narrower missing-module trace.
- An operator could extract client state and use unsupported external tooling;
  that is not an official recovery operation.

## Weakest links

1. **A1 (`code`/deployment)** — formation capability and endorsed-FMan policy
   determine which valid configurations an FI can obtain.
2. **L1 (`enum`/`code`)** — all public acceptance checks must remain free of a
   module-capability gate for the witness.
3. **L2–L3 (`code`)** — worker ordering and target-client API behavior are the
   irreversible boundary.
