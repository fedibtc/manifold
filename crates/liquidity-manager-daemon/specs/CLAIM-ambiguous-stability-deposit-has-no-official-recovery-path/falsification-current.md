# Current counterexample

A crash can occur after FLIP durably records a stability item as `submitting`
and the target client accepts `deposit_to_provide`, but before FLIP records the
returned operation id. On restart FLIP refuses to guess whether it submitted
and moves the item to `action_required`, which prevents a duplicate deposit.

The official retry and cancel paths cannot resolve this item when its attached
provider-wallet funding operation is already completed. FLIP has no workflow to
identify the unrecorded target-client operation, resume it, or sweep the
associated e-cash. The provider's value is then operationally stranded, not
cryptographically unrecoverable: the allocation stays reserved and needs
out-of-band target-client intervention.
