# CLAIM-liquidity-manager-managed-funds-not-lost: FLIP does not lose funds under its management

Within the documented production envelope, FLIP does not lose funds through an
operational custody failure while moving them among its configured funding
wallet, attributable gateway credit, FLIP-managed target-client ecash, and
FLIP-managed authority over stability-pool positions. Every FLIP-triggered
principal or fee debit is authorized by the configured or authenticated
operational authority, reaches the exact authorized target, and stays within its
durable bound; retries cannot duplicate it; automatic
terminal and accounting transitions retain the credentials and item-specific
evidence needed for supported recovery or reconciliation after cancellation,
crash, restart, or restore.

This property concerns the daemon's facilitation and technical custody. It does
not assert who legally or economically owns funds merely because the deployment
holds their credentials. It does not assert that a stability provider's
protocol-authorized leveraged settlement preserves principal, or that a gateway,
federation, or stability product is solvent and redeemable. Those are ownership,
product-risk, and dependency properties rather than operational-custody results.

## Assumptions

- Every admitted allocation and operator withdrawal has one durable semantic
  intent whose configured or authenticated operational authority fixes its
  source, target, principal, fee bound, and reservation; distinct intents cannot
  reuse consumed authority.
- Every irreversible managed-funds effect, including one whose item never
  completes, uses the exact source, target, principal, and fee bound of its
  durable semantic intent, and aggregate actual effects and charges for that
  intent cannot exceed those bounds.
- Every automatic terminal or accounting transition retains the managed custody
  credentials and durable item-specific evidence for the supported recovery or
  reconciliation path; only the documented authenticated audited abandonment
  may deliberately relinquish FLIP's management authority.
- [CLAIM-allocation-completion-has-attributable-provider-outflow](CLAIM-allocation-completion-has-attributable-provider-outflow.md).
- [CLAIM-duplicate-operator-withdrawal](CLAIM-duplicate-operator-withdrawal.md).
- [CLAIM-duplicate-stability-deposit](CLAIM-duplicate-stability-deposit.md).
- [CLAIM-manual-safe-to-retry-duplicates-provider-send](CLAIM-manual-safe-to-retry-duplicates-provider-send.md).
- [CLAIM-fi-stale-capacity-reuse](CLAIM-fi-stale-capacity-reuse.md).
- [CLAIM-wallet-budget-overcommit](CLAIM-wallet-budget-overcommit.md).
- [CLAIM-post-cancellation-effect](CLAIM-post-cancellation-effect.md).
- [CLAIM-accepted-stability-allocation-requires-target-module](CLAIM-accepted-stability-allocation-requires-target-module.md).
- [CLAIM-stability-worker-config-revision-fence](CLAIM-stability-worker-config-revision-fence.md).
- [CLAIM-failed-stability-allocation-strands-ecash](CLAIM-failed-stability-allocation-strands-ecash.md).
- [CLAIM-ambiguous-stability-deposit-has-no-official-recovery-path](CLAIM-ambiguous-stability-deposit-has-no-official-recovery-path.md).
- [CLAIM-official-backup-lacks-common-recovery-point](CLAIM-official-backup-lacks-common-recovery-point.md).
- The operator protects and restores the configured funding credential,
  target-client root secret, FLIP database, and target-client databases from one
  supported recovery point, and deliberately authorizes every authenticated
  withdrawal or audited abandonment.
