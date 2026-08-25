# Proof of CLAIM-push-gateway-production-ready

## Scope

This is a compositional proof of the local implication in
[CLAIM-push-gateway-production-ready](../CLAIM-push-gateway-production-ready.md).
It grants each immediate assumption as an axiom and does not assess whether
any assumption holds in the repository. In particular, it does not inspect
the implementation or descend into future Push Gateway assumptions.

The conclusion covers the documented production mode only. It includes
recipient management, public invocation, durable dispatch, provider outcomes,
confidentiality, bounded-time delivery, and restart or restore recovery.

## Model and quantifiers

For every configured deployment \(D\), authenticated recipient \(R\), bearer
hook \(H\), registration \(G\), accepted invocation \(I\), optional
idempotency-key namespace \(N\), and the hook's active installation target \(T\)
snapshotted for \(I\):

- \(D\) has the stated production configuration. Workload and dependency
  availability satisfy the release's stated delivery preconditions.
- \(R\) may manage only resources authorized for \(R\); a public caller may
  possess \(H\)'s URL but not a recipient-management credential.
- \(I\) is accepted subject to the configured bounds. When it carries an
  idempotency key, that key belongs to \(N\), whose equality relation has the
  documented invocation-contract meaning.
- A terminal provider outcome is provider acceptance, permanent invalid-token
  handling, or an observable actionable dead letter. Provider acceptance does
  not mean application receipt.
- The confidentiality conclusion concerns the exact credentials, bearer hook
  URLs and secrets, registration tokens, recipient identity, and notification
  contents named by the claim.

The conclusion requires that every accepted invocation has one durable
admission, that accepted attempts with the same key in \(N\) share that
admission, and that its single \(T\) in the admission-time installation-target
snapshot reaches one terminal provider outcome by
the documented delivery deadline, and that service and durable state recover by
the recovery objective. These are predicates to derive, not model definitions
or preconditions.

## Assumptions

The proof uses the claim's immediate assumptions as follows:

1. **Gateway contract.** The first assumption supplies the component-local
   positive recipient-management capability and ownership bounds, exactly-one
   atomic durable admission for every accepted invocation, one shared admission
   per documented idempotency-key namespace,
   admission-time installation-target snapshots, deadline-bounded terminal outcomes and
   observable dead letters, recovery-objective-bounded restart and restore,
   and interface confidentiality.
2. **Deployment envelope.** The second assumption selects production mode, its configured
   admission profile, and its single-process configuration, limits, protected operator boundary, and
   stated workload, availability, deadline, and recovery quantifiers.
3. **Persistence and operations.** The third assumption supplies transaction,
   durability, confidentiality, migration, backup, restore, and stopped-service
   outbox-administration conditions.
4. **Public transport and time.** The fourth assumption supplies the public
   URL and source interpretation needed at the TLS/proxy boundary, the
   confidentiality of credentials, bearer hook URLs and secrets, registration
   tokens, recipient identity, and notification contents at that non-gateway
   hop and its observability surfaces, and the clock conditions used by
   authentication freshness and retry timing.
5. **Provider boundary.** The fifth assumption supplies the configured FCM
   provider, expressly including its OAuth credential service, and their
   documented behavior and confidentiality boundary.
6. **Secret and bearer possession.** The sixth assumption supplies the
   confidentiality of the listed secrets and the authority conveyed by a
   bearer hook URL.

## Argument

1. **[assumption] Ownership and bounded management.** Gateway contract gives
   every authenticated recipient the positive ability to create, manage, and
   revoke its own bounded hooks and registrations, and denies those actions for
   other recipients. Public transport and time preserve the documented request
   identity semantics, while secret possession distinguishes an authorized
   bearer invocation from recipient management. Thus neither a different
   authenticated recipient nor an unauthenticated management caller gains the
   claim's management authority.
2. **[assumption] Admission, idempotency, and target meaning.** Gateway
   contract supplies one atomic durable admission for every accepted invocation;
   all accepted attempts with every key in its documented namespace share that
   admission, which captures the hook's one active installation target.
   Persistence and operations make that
   admission durable. Therefore later registration changes, retries, or
   restart do not change which \(T\) the conclusion quantifies over or create
   another durable admission for the same key in \(N\).
3. **[assumption] Restart and restore.** Gateway contract supplies recovery
   of service and its durable state within the recovery objective. Persistence
   and operations preserve that state through the documented database and
   backup/restore procedure. Hence recovery resumes the admitted work within
   the claim's recovery quantifier rather than silently discarding it.
4. **[assumption] Provider terminal states and observability.** Gateway
   contract makes the snapshotted target reach provider acceptance, permanent
   invalid-token handling, or an observable actionable dead letter. Provider
   conditions give those labels their documented FCM/OAuth meaning. This proves
   the specified terminal alternatives without asserting application receipt.
5. **[assumption] Confidentiality.** Gateway contract covers the gateway's
   public, persistence, operator, and observability interfaces. Persistence
   and operations extend that boundary to the production database and backups;
   public transport covers the TLS/proxy callers, logs, errors, metrics, and
   operators for every data class the conclusion names; provider terms cover
   FCM and its included OAuth service; and secret confidentiality excludes
   unauthorized possession. Their conjunction covers callers, logs, errors,
   metrics, backups, operators, and FCM without adding an unlisted disclosure
   channel.
6. **[assumption] Delivery deadline.** The deployment envelope limits workload,
   states dependency availability and the delivery deadline. Public transport
   and time provide the timing interpretation, gateway contract supplies
   deadline-bounded progression and classification under those preconditions,
   and provider conditions define the external outcomes. Consequently every
   \(T\) reaches one of the stated outcomes within that deadline.

The six steps cover every conjunct of the conclusion: management authority
and resource bounds (1), idempotent durable admission and the installation-target snapshot
(2), restart and restore (3), delivery outcomes (4), confidentiality (5), and
the workload/deadline quantifier (6). Their shared deployment envelope binds
them to one documented production mode, so no conclusion conjunct is left as
an unstated operating condition.

## Completeness

The argument considers in-scope scenarios in which all six assumptions hold but
the conclusion fails:

- ownership or authentication confusion, including bearer-hook authority;
- unbounded hook or registration creation;
- rejection of all authorized management requests;
- non-atomic enqueue, duplicate idempotency admission, or a changed installation target after admission;
- lost work across restart, restore, migration, or outbox administration;
- an unclassified provider result, a target that remains pending, or a dead
  letter that is not observable and actionable;
- disclosure through callers, logs, errors, metrics, backups, operators, FCM,
  OAuth, or a public transport boundary; and
- workloads, dependency outages, clock behavior, or recovery that violate a
  stated deadline or objective.

Each scenario contradicts Gateway contract or one of the environment,
persistence, transport, provider, or secret assumptions it explicitly
depends on. No remaining scenario falsifies the local implication while every
immediate assumption is granted.

## Residuals

This proof does not cover a deployment outside production mode; workloads or
dependencies outside the release's stated preconditions; multiple active
processes for one database; application receipt after provider acceptance;
compromise of a listed secret; or an operator, database, backup, TLS/proxy, or
configured FCM/OAuth service outside its stated assumption. These cases are
outside the claim's quantifiers, not exceptions within them.

## Weakest link

Gateway contract is the least mechanically enforced premise because it
compresses the component-local behavior that future focused Push Gateway
claims should establish. It is intentionally a direct premise here: replacing
it with lower claims would start the next bounded series step and is outside
this claim's scope.
