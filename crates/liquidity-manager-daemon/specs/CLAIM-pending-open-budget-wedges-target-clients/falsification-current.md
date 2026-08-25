# Current counterexample

Four unanswering target-client opens fill `MAX_PENDING_OPENS`. Every later
`create_or_load` for an uninstalled federation returns
`TargetFedimintError::OpensAtCapacity` until the daemon restarts. The separate
pending budget bounds the retained leak but does not preserve admission
progress.

See [the current argument](proof.md) for the preserved premises, residuals, and detailed derivation.
