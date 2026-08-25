# CLAIM-pending-open-budget-wedges-target-clients: Four unanswering targets stop FLIP opening any target client

Pending target-client opens cannot consume the whole open budget and prevent every other target client from opening.

## Status

Falsified: four unanswering target-client opens fill `MAX_PENDING_OPENS` and
block every new target client until the daemon restarts; the separate pending
budget bounds the retained leak but does not preserve admission progress.

## Assumptions

- The documented FLIP deployment and operator interfaces are used.
