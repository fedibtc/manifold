# Current counterexample

Two concurrent `restore_backup` calls can both stage their archives and pass
the second empty-directory check before either moves data. Their subsequent
moves place contents from different archives into the same data root.

See [the current argument](proof.md) for the preserved premises, residuals, and detailed derivation.
