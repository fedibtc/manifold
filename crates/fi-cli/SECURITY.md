# fi-cli security

## Scope and trust model

`fi-cli` is development and test tooling for exercising `fi-client` and
end-to-end federation flows. It is not a supported production FI application,
wallet, or operator interface. Run it as an ordinary local user in an isolated
development/CI environment, use test federations and test funds only, and treat
`--state-dir` as disposable.

The local operator, command-line arguments, configuration paths, and ordinary
identity/database state are inside this tool's trust boundary. Ordinary state
relies on the operator's directory permissions and filesystem; it is not
designed to resist a malicious local user, same-user process, symlink race,
concurrent invocation, or crash at an arbitrary filesystem operation. Identity
reads are bounded to the fixed 32-byte format so accidental corruption cannot
cause an unbounded allocation. Delete and recreate broken test state rather
than treating it as recoverable production data.

Any future production or real-funds use, shared or untrusted local environment,
concurrent invocation, or recovery guarantee must re-evaluate state-root
ownership and modes, no-follow operations, atomic identity publication, parent
directory durability, RocksDB path anchoring, and state migration.

Wallet root secrets, funding tokens, and DKG completion callback URLs remain
different: they are explicit secret/bearer inputs that can be exposed
accidentally through process metadata, logs, public files, or interrupted
imports even in development. Their hardened input and journal contracts below
remain mandatory. Do not supply production wallet secrets, bearer tokens,
invites, or funds to `fi-cli`.

The existing funding-token journal deliberately retains stronger no-follow,
inode-fencing, no-clobber, and crash-durability behavior from its separately
reviewed bearer-token import contract. That behavior is outside this ordinary
identity/state re-evaluation and must not be weakened without an explicit
change to its threat model.


## Formation timing input

Pinned formation constructs its poll interval and per-driver invocation timeout before
reading wallet-secret or setup-payment-policy input and before opening or
creating identity, FI database, wallet, Iroh endpoint, or other network
resources. Zero values and values outside fi-client's shared native/WASM timer
domain fail with a sanitized invalid-formation-options error. A process regression
verifies that rejected timing input leaves `--state-dir` absent. Re-check this
ordering when adding pre-dispatch CLI input handling or formation resources.


## DKG completion callback bearer input

Pinned formation accepts a push-gateway hook URL only from the file named by
`--completion-callback-url-file`; the URL itself must never appear in argv or
shell history. On Unix, `fi-cli` opens the file without following symlinks and
requires an owner-owned regular file with permissions exactly `0600`. It reads
at most 2,050 bytes, permits only a trailing LF or CRLF beyond the protocol's
2,048-byte URL maximum, and rejects the file before creating or opening FI
state. Temporary input bytes zeroize on drop. Errors, debug output, and normal
CLI output must not contain the URL or its hook secret. Remove the file after
use.

The paired idempotency key is a deduplication identifier rather than the hook's
authorization secret. The resulting typed callback redacts both fields and is
handed only to the pinned formation driver; registry-selected formation rejects
callbacks before state access.


## Wallet root secret input

`fi-cli` accepts a payment-wallet root secret only from a secure file named by
`--wallet-secret-file PATH`, or by the `FI_CLI_WALLET_SECRET_FILE` environment
variable when the option is absent. The option wins when both are set. The
environment variable contains only a path, never secret material.

```console
install -m 0600 /dev/null "$wallet_secret_file"
secret-command >"$wallet_secret_file"
fi-cli create … --wallet-secret-file "$wallet_secret_file"
```

The file must be a regular file owned by the current user with permissions
exactly `0600`. `fi-cli` opens it without following symlinks, validates the
opened file, and performs a bounded read before opening local state or making
network connections. This workflow is supported on Unix; `fi-cli` rejects
wallet-secret file input on other platforms. Remove the file after use.

Encoded and decoded root-secret buffers cannot be formatted, produce only
sanitized errors, and zeroize when dropped immediately after the wallet opens.
Derived wallet state necessarily remains live while the wallet is open.

`payment-wallet join` is the only direct-funding command that accepts an
invite. Follow-up balance, deposit-address, and Lightning funding commands
reopen only a previously initialized per-federation database selected by its
federation id; they cannot join a new federation without an invite. Every
invocation holds the existing exclusive wallet-root lock, so direct funding
cannot race formation payment or another wallet command in a second process.
Private API-secret-bearing invites are supported for testing. Like other
command-line arguments, they are trusted local inputs rather than hardened
secret transport and may be visible in shell history, process metadata, or
local dependency logs. Use only test credentials.

Wallet deposit addresses and BOLT11 invoices are public receive artifacts
and are intentionally printed to the invoking terminal. They must still use
test funds and may reveal test transaction metadata when copied or recorded.
Lightning invoice creation also prints its non-secret operation id so a later
`await-invoice` invocation can resume the persisted Fedimint receive state.
The client prefers wallet-v2 and LN-v2 when the joined federation supplies
them, falls back to legacy wallet when wallet-v2 is absent, and falls back to
legacy LN only when LN-v2 explicitly reports that no gateway is available
before creating an operation. Other LN-v2 failures cannot start a second
receive path. `await-invoice` dispatches by the durable operation's module kind.
Wallet-v2 deposit-address derivation has a caller-visible bounded deadline; a
timeout returns a sanitized error without authorizing or spending formation
funds.
On-chain waiting subscribes to the persisted wallet's primary Bitcoin balance
and succeeds only at an explicit minimum; a deadline returns an error without
changing payment authorization. Neither funding path opens FI identity/state,
selects FMans, or authorizes formation spending.

`payment-wallet remit-guardian-fee` is likewise test-only. Its explicit
BtcDepositor account id and pre-sealed metadata file are trusted local inputs;
the command validates only the module account type and submits the real
stability-pool deposit. It does not authenticate the recipient, derive the
guardian split, inspect the sealed payload, or enforce production payer policy.
Supplying the wrong account irreversibly sends the test funds there. Use only
isolated test federations, test account ids, test metadata, and test funds. The
command shares the wallet spend guard and refuses while a locked-payment hold
or its balance floor is active. If the recipient deposit commits but the
wallet's change output fails, the error includes the committed operation id and
must not be retried.

The previous `--wallet-secret-hex VALUE` and `--wallet-secret-stdin` interfaces
are intentionally rejected. Store `VALUE` in a mode-`0600` file and pass its
path with `--wallet-secret-file`, or set `FI_CLI_WALLET_SECRET_FILE` to that
path. Never put the secret value itself in an argument or environment variable.

This policy covers secret-input transport and in-process ownership only.
Production-grade ordinary state, identity, wallet, and filesystem hardening
remains outside this development/test tool's scope.


## Reference FI payer recovery

The CLI's `fi-client` payment adapter is the development/reference FI payer;
the FMan wallet is payee-only. `fi-client`, not this adapter, schedules initial,
resumed, and replacement payments. Before `fi-client` starts those locked
payments one at a time,
the payer journals one deterministic aggregate reservation binding the payer,
ordered unique quote ids, exact foreign-output plan hashes, fee-aware logical
debit allocations, and the required balance floor. It derives each allocation
by independently dry-running that exact transaction, so the total covers its
quoted outputs and current Fedimint fees without pre-selecting disjoint physical
notes. The reservation capability is
required to start each member. Under the wallet-wide spend guard, the payer
first locates or atomically persists the deterministic quote-bound Fedimint
operation; it then durably advances the matching reservation member from
`Held` to `Started` before releasing the guard or awaiting transaction
consensus. A crash between those checkpoints leaves `Held` plus the exact
operation identity, which recover-only adopts without another spend. An
existing same-id journal is reconstructed and validated before any new
fundability check so a partially consumed aggregate never charges
already-started members again. Whole-aggregate release is allowed only while
no member has started.

Because one physical note may fund multiple members only through returned
change, immediately before each new member the payer quotes that transaction's
current net cost, including primary input/change fees and dust. It proceeds
only when the post-change balance will still cover every other logical held
allocation and the active balance floor; an increase over the dry-run
allocation may use only unreserved headroom. The accepted transaction's exact
change range is stored in operation metadata and awaited to primary-module
finality before the adapter returns. A larger input may therefore supply the
current member and return value for later members without consuming a sibling
hold.

The adapter exposes insufficient funds to `fi-client` as a value-safe selected
payer retry only when the wallet's explicit balance comparison fails before a
new journal is written. That proof is a private typed error marker carried
through contextual errors. Planning failures, an existing same-id binding
mismatch, database/commit failures, and any lost result after journal creation
remain generic payment errors so `fi-client` retains the formation and retries
the exact deterministic id.

The adapter exposes a separate recover-existing-only aggregate probe. It reads
the wallet journal under the spend guard, validates the payer, reserve floor,
ordered quote ids, and exact output-plan hashes, and returns exact existing or
authoritative absence without opening, funding, or creating a reservation.
Mismatch, storage failure, and ambiguous lookup remain errors. This probe lets
`fi-client` release a journal whose successful reserve response was lost even
after selection freshness or verifier provenance changes.

The wallet-wide spend guard serializes reservation creation, payment
submission, recovery adoption, release, and ordinary out-of-band spending.
Ordinary spending must leave every held debit allocation plus the active balance
floor untouched. Every shared-row monetary mutation uses bounded database
autocommit so concurrent member transitions reload and merge after conflicts;
commit/backend failure is propagated. A member moves monotonically through
Held, Started, Terminal, and Released. Only a wallet-issued terminal proof,
bound to the journal, quote, output plan, and debit, can release it. Repeating
exact terminal recovery and release is harmless; a Prepared or ambiguous
member cannot be released.

Consensus rejection alone is not terminal ownership proof. Fedimint starts
automatic mint-input refund transactions only after the original transaction
rejects. Recovery identifies the original Created transaction by its exact
metadata txid, joins unordered and duplicate same-operation transaction updates
by txid, and requires accepted refund transactions to cover the original
primary-input multiset exactly. Mint-v1's rejected bundle followed by per-note
fallbacks is therefore handled without inspecting private mint state machines.
Every output of the accepted covering transactions must become spendable before
the member advances from `Started` to `Terminal`; timeout, partial coverage,
overlap, malformed siblings, or output failure leaves `Started` for exact
restart recovery and cannot authorize replacement.

Every locked-payment operation is identified by the exact canonical quote,
mint generation and module, and quoted issuance set. Non-secret operation
metadata, including the quoted-output and payer-change ranges, is committed
atomically with funding. Recovery probes only that quote-bound identity and
requires both ranges in its exact metadata. Any mismatched quote, module,
generation, output plan, debit, or reservation fails closed. Neither pay nor
recovery reports a funded member until consensus accepted that exact
transaction and the primary module made its payer change spendable.

Recover-only paths never create, fund, or submit a transaction. `Absent` is
safe only after the wallet authoritatively proves the exact operation does not
exist. `Rejected` is safe only after terminal consensus rejection, exact
automatic input-refund recovery, spendable refund outputs, and durable
terminal-proof issuance. A pay path recovers first; ambiguous submission must
inspect the deterministic operation before returning and must never authorize
a second spend just because its caller lost the first result.

Refund output secrets are reconstructed deterministically from the wallet root
and exact quote binding and stay in non-serializable contexts. Repeating the
same FMan-signed refund must converge on the same outputs and ordinary balance;
the transaction, module, output count, and prepared issuance are validated
before submission or finalization.

Changes to this adapter require drop/reopen coverage for partially started
aggregates and a whole release whose FI checkpoint was lost; exact-match and
required-range recovery; unordered/duplicate rejected-input-refund replay,
mint-v1 per-note fallback, and spendable refund outputs; duplicate quote,
plan/debit
mismatch, concurrency, and idempotent terminal replay; no-spend recover-only
behavior; one-note sequential settlement; and repeated refund
submission/finalization.

This pre-production CLI accepts only its current reservation and operation
metadata schemas. Older test state is not migrated; incompatible rows fail
closed and require a fresh disposable test wallet.


## Registry selection and post-formation operations

Registry-backed `create` retains the verified, non-serializable selection
approval only for the current process. It completes discovery and verification
before opening identity, FI state, or the payment wallet, then consumes the
approval directly through `fi-client`; it never converts selected rows into
pinned locators. Paid selected creation requires an explicit aggregate cap.

`--insecure-skip-fman-trust` is an explicit development/staging escape hatch
for incomplete test credential publication. It retains event/document
authentication, freshness, intent compatibility, advertised capacity, dialing,
and every live quote/response verification, but deliberately skips
HolderAuthorization and PeerBadge admission by entering the diagnostic pinned
driver. The CLI refuses it under the production profile. Its output must never
describe those FMans as verified or selected.

Liquidity commands use only a formed federation already held in `--state-dir`.
Provider discovery remains the library's no-private-data gate; the CLI passes
an admitted provider identity back to `fi-client`, which refreshes admission
before it discloses the invite to the admitted provider or persists and submits
a request. Because a persisted formed snapshot reopens unsynced, both a new
request and durable-operation recovery reconcile the formation and invoke the
library liquidity operation through the same open client. If reconciliation
fails, the subsequent liquidity future is never polled: a new request is not
persisted or disclosed to a provider, and a durable operation is neither
status-queried from its provider nor replayed.
JSON may contain the provider endpoint, exact operation hash, and allocation
status because they are required test outputs. It must never add FI secret-key
bytes, wallet secrets, bearer ecash, payment signatures, or refund contexts.
Use these commands only with test federations and test providers.

Metadata and guardian-fee maintenance reopen only the active formed federation,
reconcile it through `fi-client::resume`, and invoke the corresponding typed
fi-client operation in that same process. Invalid metadata values, fee timing,
rates above the shared payer ceiling, and malformed FI account files fail before
identity, durable FI state, consensus, or Iroh access. The fee account is the
public serialized `spv2.our_account(AccountType::BtcDepositor)` descriptor from
the FI consumer's client for this formed federation, not its spend key. The CLI
cannot prove derivation or key control from that public descriptor: another
valid account redirects the FI share, so only test accounts controlled by the
operator belong here. fi-cli installs it in an explicitly test-only
`FiFeeAccountProvider`; `fi-client` selects the persisted formed federation id
and invokes that capability, while the fee operation itself accepts only the
rate. Production consumers must resolve the exact joined client instead of
using such an override. fi-cli never derives or accepts guardian accounts or the
Guardian Verification Fee account and never parses the committed recipient-list
wire shape. A successful result is printed only after
fi-client's fresh threshold-consensus readback verifies the exact requested
metadata value or complete library-derived fee mapping. JSON may contain the
public consensus key/value, fee rate, and FI account id, but no FI identity
secret or account spend key.


## Funding bearer-token journal

`--funding-token-file` accepts only an owner-owned regular file with exact mode
`0600`. The CLI opens the file without following symlinks, caps it at 256 KiB,
and retains the validated descriptor while moving and reading the journal. It
uses one opened parent-directory descriptor for journal transitions, rejects a
replacement before deletion, and syncs that directory after creating or
deleting the journal.

The option is available on `create`, `resume`, and `authorize-payments` so an
interrupted formation can be funded without restarting it. In every case it
requires the selected payment federation, wallet coordinates, and a matching
invite before import.

Before publishing the restart-journal name, the CLI syncs the validated token
file and atomically renames it without replacing an existing journal. It then
syncs the parent directory even if subsequent validation fails, reopens the
published name, and verifies that it still identifies the retained inode.
Verification failure is fail-closed after the directory transition has been
made durable. Restart also syncs an existing validated journal before wallet
use. This ordering assumes the underlying filesystem implements file and
directory `fsync` plus atomic no-replace rename with its documented crash
guarantees.

An interrupted receive reuses the validated `*.fi-cli-in-progress` journal.
Mint-v2 receive recovery derives the same deterministic operation id from the
retained token, awaits its accepted transaction, reloads and validates its
durable receive metadata, and waits for every primary-module reissue output to
be spendable before deleting the journal or permitting formation preflight.
Keep its directory inside the same operator trust boundary as the token; this
guarantee does not harden general CLI state or identity paths.

The atomic no-replace operation is platform-specific: Linux and Android use
`renameat2(RENAME_NOREPLACE)`, while Apple platforms use
`renameatx_np(RENAME_EXCL)`. Other Unix targets fail closed as unsupported.
Any platform-support change must preserve the no-clobber and retained-inode
contract and run the funding-token journal tests on that platform.
