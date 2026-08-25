# CLAIM-ambiguous-stability-deposit-has-no-official-recovery-path: Ambiguous stability deposit has no official recovery path

Every ambiguous stability deposit has an official recovery path that preserves provider value.

## Status

Falsified: a crash after the target accepts `deposit_to_provide` but before FLIP
records its operation id moves the item to `action_required`; that state prevents
guessing and duplicate submission but has no official value-preserving
resolution path.

## Assumptions

- The documented FLIP deployment and operator interfaces are used.
