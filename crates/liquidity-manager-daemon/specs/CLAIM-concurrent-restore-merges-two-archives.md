# CLAIM-concurrent-restore-merges-two-archives: Two concurrent restore-mode restores merge two archives into one data root

Two concurrent restore operations cannot merge different archives into one data root.

## Status

Falsified: two concurrent `restore_backup` calls can both pass the second
empty-directory check and then move different staged archives into the same
data root.

## Assumptions

- The documented FLIP deployment and operator interfaces are used.
