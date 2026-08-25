# CLAIM-stability-worker-config-revision-fence: Stability worker config revision fence

A stability-pool allocation that FLIP accepted for config hash `H` cannot cause
provider-wallet outflow unless the address it funds was minted by a target
client that had config hash `H` and a stability-pool module.

The FI controls its endorsed federation's later consensus configuration and
scheduling. It cannot write FLIP storage or act as Admin.

The claim does not require the target client to still carry `H` when the send
settles. Between the mint and settlement the pool can close
and reopen the client, and under A3 the reopened instance can carry `H2`. That
does not move the value — the address is committed by then, and L4's single
writer means a retried send reuses it — so the property that matters for A2 is
the mint-time one.

## Status

Unverified.

## Assumptions

- **A1.** A federation id does not commit the complete client module map; the
  canonical config hash does.
- **A2.** A provider-wallet withdrawal to a claimed target peg-in is an
  irreversible provider outflow.
- **A3 — pinned config writers.** *Reopen.* Every client build spawns a task that
  refetches the federation's client config and, on any difference, writes
  `PendingClientConfigKey` into the client's own database.
  `ClientBuilder::open` promotes that pending config over `ClientConfigKey`
  before reading it. So a close and reopen of one database can present a
  *different* config than the instance just closed.
  `ClientBuilder::validate_config_update` rejects any global change, module
  removal, or module mutation, so a promoted config's module map is a superset
  of the one it replaced.

  *In place.* The live config of an open client has exactly one writer,
  `Client::get_or_backfill_broadcast_public_keys`, which assigns
  `*self.config.write().await` and writes `ClientConfigKey` directly, without
  `validate_config_update`. It is reached only from the API-announcement and
  guardian-metadata refresh tasks that `build_stopped` spawns for every client,
  and only when the stored config has `global.broadcast_public_keys: None`.

  *Why it is consistent with the other assumptions.* A1 and the
  module-addition residual decline to trust the target federation's
  *configuration*, which the FI controls unilaterally and may change at will.
  A4 is a different kind of statement: it is about the guardian software, which
  needs threshold-many guardians to agree, and the FI is one participant rather
  than the threshold. A federation whose threshold runs modified `fedimintd` is
  outside the admitted trust envelope, and no property of FLIP's own code
  can substitute for that — the value being protected sits in a federation whose
  guardians can move it by consensus regardless of this fence.
