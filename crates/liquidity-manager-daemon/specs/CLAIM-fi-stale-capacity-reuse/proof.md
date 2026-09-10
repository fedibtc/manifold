# Current argument

## Argument

Current wallet observations begin with a durable read tick; older reads cannot
overwrite newer observations. Settlement writers stamp their durable ordering,
release ordering is computed when accounting evaluates released items, and
active outgoing accounting covers every outgoing operation type. It retains
terminal sends with non-null settlement metadata until a strictly later
observation.
Public admission reads the durable observation and accounting in its serialized
transaction, so the former pre-debit-read counterexample no longer applies.

## Unresolved proof obligations

A complete verification still needs an enumeration of every terminal, manual,
restore, and legacy-null settlement path. No current FI-only uncharged-debit
counterexample has been established, but this argument does not yet prove none
exists.

## Weakest links

Legacy terminal rows with null settlement metadata and every operation-reset path
need explicit classification against the strict read-tick rule.
