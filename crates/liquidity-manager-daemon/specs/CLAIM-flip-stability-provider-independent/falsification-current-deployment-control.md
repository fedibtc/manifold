# Current deployment does not represent independent stability-provider control

At source commit `db4e6bf5` (the base of this documentation change), an ordinary
official setup has one configured
gatewayd-backed funding wallet. An accepted stability allocation reaches the
shared funding-withdrawal path, so this wallet supplies its principal.

For the target federation, FLIP generates and stores a mnemonic in its managed
target-client database and builds the client from that root secret. The
stability submission uses `deposit_to_provide_with_operation_id`, and the same
client derives `our_account(AccountType::Provider)`. The supported setup,
request, and schema carry no separate stability-provider funding credential,
provider-account key, beneficiary, or delegation binding.

Both immediate assumptions remain granted. This establishes absence of a
separately represented technical authority, not legal or beneficial ownership:
the deployment may use its own capital or act under an external agency agreement
which the daemon does not represent or enforce.

The source witness is `SetupConfig` and `RequestLiquidityRequest` in
[`admin.rs`](../../../service-liquidity-manager/src/admin.rs) and
[`public.rs`](../../../service-liquidity-manager/src/public.rs), together with
[`stability_allocation.rs`](../../src/stability_allocation.rs),
[`allocation_funding.rs`](../../src/allocation_funding.rs),
[`wallet.rs`](../../src/wallet.rs),
[`target_fedimint.rs`](../../src/target_fedimint.rs), and
[`stability_pool.rs`](../../src/stability_pool.rs). This is a source-derived
representation counterexample, not an executed loss scenario or a conclusion
about an operator's external agreements.
