# Current argument

## Argument

New submissions commit a caller-owned operation id and immutable tuple before the
external call, so restart can query and resume the same operation. For legacy or
unbound `action_required` state, authenticated inspection can locate candidate
operations and `bind_target_deposit` validates and records one before resuming.
The former lost-return-id counterexample is obsolete.

## Unresolved proof obligations

The record does not precisely define every “ambiguous” state or whether preventing
new ambiguity satisfies its universal wording. Existing ids cannot be replaced,
and the documented external runbook recovers mint ecash but not value already in
the stability pool. The available interfaces have not been shown sufficient for
every conflicting, legacy, or already-pooled case.

## Weakest links

A precise ambiguity domain and an exhaustive mapping from each state to a
value-preserving official recovery path are still needed.
