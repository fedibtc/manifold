# Current counterexample

A crash after the target accepts `deposit_to_provide` but before FLIP records
its operation id moves the item to `action_required`. That state prevents
guessing and duplicate submission but has no official value-preserving
resolution path.

See [the current argument](proof.md) for the preserved premises, residuals, and detailed derivation.
