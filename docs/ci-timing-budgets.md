# SelfCI timing budgets

SelfCI times the verification pipeline, not just test execution. The
`cargo/nix cargo checks` step includes Nix evaluation, downloads, builds, and
tests when the corresponding outputs aren't already available. These budgets cover routine Linux checks; they are not a cold-build SLA or
a benchmark for individual tests. Nextest's test timeouts are unchanged.

| Scope | Slowest confirmed successful measurement or upper bound | Hard limit |
| --- | ---: | ---: |
| Whole run | 32m 39s for the enclosing GitHub Actions step | 45m |
| Any job (including `cargo`) | 30m 22.057s | 41m |
| Any step (including `nix cargo checks`) | 30m 22.025s | 40m |

The `main` dispatcher starts the child jobs but doesn't wait for them.
Job time excludes queueing; the whole-run clock includes preparation and, in
the merge queue, publication and hooks. Enclosing limits still apply when a
job or step starts late.

## Calibration

The job phase took as little as 42s in the November 17 sample, but 30m
22.146s in the [slowest confirmed run][slow-run] in the wider window.
To find the long tail, we screened 331 successful GitHub Actions SelfCI
workflow records on self-hosted Linux runners, created October 18–November
17, 2026 (UTC), by workflow elapsed time. The records represent current attempts, not every attempt of
rerun workflows. Workflow elapsed time is only an upper bound on SelfCI time:
it can include runner waits, setup, and cleanup. We checked the logs and
the `Run selfci` step timestamps for the longest workflows until all the
remaining workflow bounds were below the longest confirmed SelfCI phase.
For example, the [longest workflow][long-workflow] took about 48m, but its
`Run selfci` step took 5m 35s; the rest was outside SelfCI.

The 40m step cap gives about 32% headroom over the longest confirmed cargo
step. The 41m job cap leaves time outside that step; the 45m run cap also
leaves room for preparation, scheduling, and cleanup. These timings
don't cover local cold stores or merge-queue hooks. Nix cache misses may
explain some of the slow runs, but the summaries alone don't establish
what rebuilt. Lint uses the shared hard limits: [one successful run][slow-fmt]
spent 142s on `cargo fmt`, so the fast November 17 timings are not a safe
lint cap. Failed runs inform diagnosis, not the successful baseline.

[slow-run]: https://github.com/fedibtc/manifold/actions/runs/41399513465
[long-workflow]: https://github.com/fedibtc/manifold/actions/runs/43094239845
[slow-fmt]: https://github.com/fedibtc/manifold/actions/runs/41401029488

## Investigating a warning or timeout

The 9m run warning flags work that exceeds the fast path without failing
legitimate slow checks. It works without history. All absolute limits apply from the first run. SelfCI reports when a scope crosses its local 90th-percentile
runtime. The configured `+25%` thresholds add a warning with investigation
guidance for each run, job, and step. The baseline uses the last 30 days of
matching successful timings. With no samples, SelfCI reports warm-up and
skips relative thresholds; with even one sample it can warn. Ten samples
is the minimum for *relative failures*, not for warnings. No relative
threshold fails CI here, even after ten samples.

Keep history in the local or merge-queue checkout. GitHub Actions cleans
the checkout, including ignored timing files, so those checks use the
absolute warning and limits without a baseline from previous runs.

Check the named scope in [the CI script](../.config/selfci/ci.sh) and compare
representative runs in the ignored `.config/selfci/timings.local.jsonl` file.
Use `nix run .#selfci-check -- --print-output` to see what Nix does in a
normal check. Cache state, host, load, and local check versus merge-queue mode
can differ without splitting the baseline. Load can explain a slowdown,
but it doesn't adjust the threshold. A warning is a reason to investigate, not
proof of a test regression.

Look for unnecessary build work, contention, or slow/stuck tests before
increasing a limit. Recalibrate using successful comparable runs only after
understanding the change. Record why if the workload changes enough to mark
old samples stale (`selfci timings stale`, using the project's pinned SelfCI);
this keeps the old measurements for diagnosis. Don't manufacture samples or
reset history just to silence warnings.
