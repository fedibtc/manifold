# CLAIM-fi-client-production-ready: FI client is ready for production

Within its documented feature envelope, when the selected eligible FMan set,
consumer adapters, and required trust services return successful valid responses
within the configured timeouts and the guardians complete DKG, `fi-client` forms
one recoverable federation within the configured formation deadline. Otherwise
it reports a typed failure without publishing `Formed`. Interruption, restart,
retry, stale quotes, concurrent drivers, and untrusted FMan or relay input do not
cause duplicate or unauthorized value commitment, acceptance of unauthenticated
lifecycle facts, rollback of admitted payment policy, or publication of
`Formed` before every selected seat agrees on the federation and its
seat-binding directory. After a supported interruption, resume reaches formation
or a typed actionable failure within the release's documented recovery
objective when the availability preconditions hold again.

For supported post-formation metadata maintenance, a freshly `Formed` FI with
a genuine consensus reader and a threshold-live guardian set either observes
the exact typed value in fresh consensus before reporting success or returns a
maintenance-specific wrong-state, terminal-refusal, or bounded convergence
failure. Concurrent whole-object changes are preserved by exact-base rebasing;
within one live invocation, same-base retries do not resubmit to guardians that
already acknowledged the mutation. Cancellation, lease takeover, and restart
cannot let a stale driver begin another wave or turn one guardian
acknowledgement into consensus; reopen is safe exact replay and may resubmit
rows. Each FMan also pins the one target admitted for the live consensus
occurrence, recording the pin before its
fallible child submit call so an ambiguous response cannot reopen that
occurrence to a conflicting target; revision-bound bases make an exact
`O -> B -> O` recurrence a fresh occurrence that stales old handlers, and a
superseded occurrence's pin is simply replaced.
This fences Iroh handlers reordered
before seat-queue entry while permitting exact replay; FI and FMan enforce the
shared 1,048,576-byte complete-object ceiling before resource-amplifying work.

For supported post-formation liquidity, `fi-client` discloses the invite only
after fresh provider and FMan trust admission, persists the exact semantic
request identity before its first mutating RPC, and recovers a lost response by
status lookup before exact replay. It exposes provider-authoritative item
states without promoting liquidity into formation success, inventing a stronger
completion state, or automatically retrying `action_required`/send-once work.
The consumer remains responsible for scheduling resume and independently
verifying the claimed attachment through the joined federation.

## Status

Unverified. The paired
[proof](./CLAIM-fi-client-production-ready/proof.md) scopes its property to an
earlier feature envelope that excludes post-formation maintenance and
liquidity attachment; the maintenance and liquidity paragraphs above await
proof re-derivation over the extended envelope.

## Assumptions

- The consumer supplies one stable FI signing identity, protects and namespaces
  the database, preserves it across supported restarts, and backs it up with the
  identity and wallet recovery material needed to resume.
- The payment adapter durably reserves each exact aggregate by deterministic
  id, makes each funding operation recoverable before committing value,
  recovers before creating a replacement, replays exact quote-bound payments
  and refund context, reports terminal rejection accurately, settles signed
  refunds idempotently, and releases only wallet-proven-safe members.
- The consensus-read adapter performs a genuine threshold Fedimint consensus
  read and does not substitute cached or fabricated configuration or metadata.
- The transport and FMan implementations honor the authenticated RPC, signed
  quote, commitment, DKG, status, and invite contracts; the pinned
  cryptographic and persistence dependencies satisfy their documented
  contracts or fail detectably.
- The deployment profile pins authentic setup-payment publishers, issuer
  identity roots, relays, and a schema-valid minimum PeerBadge trust level, and
  the required trust services and clocks satisfy the documented freshness and
  availability bounds.
- The release states supported formation sizes and workload limits,
  dependency-availability preconditions, formation deadlines, and a recovery
  objective.
- Within that envelope and those limits, `fi-client`'s durable formation state
  machine resolves concurrent drivers, restarts, and retries for the same
  consumer formation intent to one stable logical formation attempt, with no
  second active attempt for that intent; that attempt has exactly one terminal
  outcome. It gives each logical external value effect one stable identity that
  it authorizes, durably keys, and executes at most once; it accepts lifecycle
  artifacts only after phase-valid authentication and validation, never
  regresses admitted payment policy, and reaches `Formed` only from durable
  matching agreement by every selected seat on the federation and seat-binding
  directory.
- For every nonterminal logical formation attempt, `fi-client` enables its
  unique next success, failure, or timeout transition, and each enabled
  transition completes within its allocated budget. When the selected eligible
  FMan set, consumer adapters, and required trust services return successful
  valid responses within the configured timeouts and guardians complete DKG,
  valid timely inputs take the specified success transition; regardless of
  availability, timeout, invalid, and terminal inputs take a typed actionable
  terminal state that cannot publish `Formed`. The cumulative transition
  budgets from start or supported resume to terminal outcome fit the configured
  formation deadline or recovery objective.
- The consumer uses only the documented feature envelope. Pinned-locator
  formation, optional authenticated setup-payment funding, DKG, seat-binding
  publication and readback, invite recovery, read-only verified FMan discovery,
  strict PeerBadge selection at or above the environment minimum,
  registry-backed Pay-and-create, exact aggregate
  reservation, proven-safe subset replacement,
  and typed post-formation name/icon-URL/welcome-message/fixed-terms maintenance
  are included. Maintenance requires a genuine threshold consensus reader and
  threshold-live guardians before the caller's deadline unless the requested
  value is already adopted. Liquidity attachment is included as a separate
  post-formation operation; guardian-fee arrangement remains a separate stack.
