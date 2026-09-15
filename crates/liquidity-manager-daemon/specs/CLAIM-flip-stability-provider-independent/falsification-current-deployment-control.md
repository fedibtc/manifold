# Current deployment does not represent independent stability-provider control

At source change `moqkvurn`, an ordinary official setup has one configured
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
