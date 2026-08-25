# Current counterexample and work

## Failure

A crash can occur after FLIP durably records a stability item as `submitting`
and the target client accepts `deposit_to_provide`, but before FLIP records the
returned operation id. On restart FLIP correctly refuses to guess whether it
submitted and moves the item to `action_required`, preventing a duplicate
deposit.

The official retry and cancel paths cannot resolve this item when its attached
provider-wallet funding operation is already completed. FLIP has no workflow to
identify the unrecorded target-client operation, resume it, or sweep the
associated e-cash.

## Practical impact

The provider can have target-client value or a stability deposit whose status is
unknown to FLIP while the allocation remains reserved and requires out-of-band
storage/client intervention. This is operationally stranded value, not a claim
that the e-cash is cryptographically unrecoverable.

## Current exposure

The schedule requires only an ordinary hard crash in the cross-store window.
The fail-closed `action_required` behavior prevents a guessed duplicate deposit
but exposes the unresolved recovery requirement.

## Recommended fix

Provide an operator-authorized reconciliation workflow that discovers and
binds the target-client operation to the item, then either resumes verified
progress or safely recovers the target-client value. Do not restore automatic
submission from aggregate balances or an absent operation id.
