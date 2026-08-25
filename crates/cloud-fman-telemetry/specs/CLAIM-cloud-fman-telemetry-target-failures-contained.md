# CLAIM-cloud-fman-telemetry-target-failures-contained: Target failures are contained

Within documented target, stream, sample, body, concurrency, cadence, and
storage limits, one hostile, slow, stale, expired, quarantined, or unreachable
target cannot remove local readiness, cause another due target to lose its
bounded collection opportunity, present an old observation as fresh, escape
resource bounds, make shutdown detach an in-progress durability segment, or
start a queued target's connection, listing, or fetch after shutdown is observed.
A target cannot appear in a private metrics scrape whose transactional lifecycle
view is ordered after that target's quarantine commit or lease expiry.

## Status

Unverified.

## Assumptions

- The release states and the deployment enforces its supported target, stream,
  sample, body, concurrency, cadence, archive, and retention limits.
- Tokio scheduling, clocks, networking, SQLite, and the operating system satisfy
  their documented contracts or fail detectably within configured budgets.
- The orchestrator delivers shutdown and permits the documented termination
  grace period.
