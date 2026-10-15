# REQ-seat-capacity-default: Recommend a seat count the host's RAM can carry

Source: MG operator sizing and the measured per-seat memory budget.

The MG recommends a seat capacity from **available** RAM. The onboarding
offer form pre-fills it when the operator clears the automatic seat limit.
Capacity is no longer a daemon-start argument.

The normative rule: one seat per whole per-seat budget of available RAM,
capped at 8 seats. The default budget is 1,229 MiB (about 1.2 GiB). GB here
means GiB (2³⁰ bytes, the kernel's binary units). A fractional budget
truncates toward fewer seats, and a host with less than one budget available
is recommended 0 seats — too small to sell any.

| Available RAM | Recommended seats |
| --- | --- |
| < 1.2 GB | 0 |
| 1.2 GB | 1 |
| 2.4 GB | 2 |
| 3.6 GB | 3 |
| 4.8 GB | 4 |
| 6.0 GB | 5 |
| 7.2 GB | 6 |
| 8.4 GB | 7 |
| ≥ 9.6 GB | 8 |

Available RAM is `MemAvailable`, lowered to the cgroup headroom
(`memory.max` − `memory.current`) when the MG runs under a cgroup limit.

## Runtime limits

Admission sells up to the lowest of three limits on total seats:

- the operator's maximum, which only the operator writes. It may be
  automatic: no operator cap, so the two runtime limits decide. New installs
  start automatic;
- a memory limit: available RAM plus the anonymous resident memory of the
  MG's seat processes, divided by the budget. Running seats count toward
  their own budget, so they do not lower the limit as they grow. It has no
  cap of 8. Until the first sample it is 4;
- a policy limit, the setup-payment publication's `max_seats_per_mg`
  (default 12).

The publication's `seat_memory_budget_mib` replaces the default budget
([SPEC-setup-payment-federations](../../../specs/SPEC-setup-payment-federations.md)).
So Fedi can raise or lower capacity across MGs without an operator action
and without a release.

When the runtime limits shipped, a migration made the operator maximum
automatic where it was from 1 to 8. The onboarding form had pre-filled a
recommendation capped at 8, so such a value most likely was that
recommendation, not an operator choice. Values above 8 and 0 (not selling)
stay. The replaced value stays in `offer_state.pre_automatic_max_seats`, so it
can be restored. The migration replaces the offer epoch, because the first
memory value of 4 can take the last free slot.

The MG writes both runtime limits at start, on each newly admitted
publication, and every 5 minutes. The memory limit falls at once and rises
only after three consecutive samples allow it, so a transient spike does not
open and close the offer. A runtime limit below the active seat count only
stops new sales. It does not stop a seat. Each write keeps the offer-epoch
invariants of [SPEC-quote-settlement](./SPEC-quote-settlement.md).

## Measurement basis

A staging measurement on Umbrel with one sold seat on a young Signet federation after about ten hours reported 171 MB RSS for
the daemon (embedded operator UI, Iroh, Nostr, setup-payment wallet client)
plus 281 MB RSS for the seat's `fedimintd`. The original 1.5 GB/seat budget was ~5×
that resting per-seat cost: headroom for DKG and chain-scan spikes and for
`fedimintd` growth as a federation accumulates history.

The 8-seat cap reflects that beyond it the expected binding constraints are
concurrent-ceremony CPU and disk IO, for which no evidence exists yet, not
RAM. Re-measure once seats carry weeks of mainnet-like history before
treating either the per-seat budget or the cap as settled; the
young-federation measurement is the weakest part of this basis.

The budget moved from 1.5 GiB to 1.2 GiB as a conservative first step after
a 7-guardian federation under a 30-minute payment and ecash workload measured
`fedimintd` at 129 MiB peak RSS per seat with a bounded blocking pool. The
publication can move it further without a release.
