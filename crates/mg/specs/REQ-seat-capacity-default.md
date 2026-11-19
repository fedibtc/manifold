# REQ-seat-capacity-default: Sell only the seats the host can carry

Source: MG operator sizing, the measured per-seat memory budget, the seat
cost simulation's CPU benchmark, and operators whose hosts share a capped
internet plan.

The MG limits seats from **available** RAM at runtime: one seat per whole
per-seat budget, by default 1,229 MiB (about 1.2 GiB). For memory, GB means
GiB (2³⁰ bytes, the kernel's binary units); for data it means 10⁹ bytes. A fractional budget truncates toward fewer
seats. Onboarding asks for no seat count: the operator limit starts
automatic, and the operator can set one later. It does ask whether the
host's internet has a monthly data cap ([Data budget](#data-budget)), and
setup does not finish without an answer.

Available RAM is `MemAvailable`, lowered to the cgroup headroom
(`memory.max` − `memory.current`) when the MG runs under a cgroup limit.

## Runtime limits

Admission sells up to the lowest of six limits on total seats:

- the operator's maximum, which only the operator writes. It may be
  automatic: no operator cap, so the other limits decide. Setup leaves
  it automatic;
- a memory limit: available RAM plus the anonymous resident memory of the
  MG's seat processes, divided by the budget. Running seats count toward
  their own budget, so they do not lower the limit as they grow. Until the
  first sample it is 4;
- a policy limit, the effective feature policy's `max_seats_per_mg_v2`
  (default 12);
- a CPU limit: the host's measured CPU capacity divided by the
  publication's `seat_cpu_iterations_per_sec`, rounded down. Without that
  field there is no CPU limit;
- a disk limit: the size of the filesystem holding the MG data directory
  divided by the publication's `seat_disk_gb` (default 80 GB), rounded down
  ([Disk](#disk)). Without a measurement there is no disk limit;
- a data limit: the operator's monthly data budget divided by the
  publication's `seat_monthly_data_gb` (default 65 GB), rounded down. It
  binds only when the operator gave a budget in GB.

The effective feature snapshot (37710, or the legacy 37707 projection before admission)
supplies `seat_memory_budget_mib_v2`, which replaces the default budget
([SPEC-setup-payment-federations](../../../specs/SPEC-setup-payment-federations.md)).
So Fedi can raise or lower capacity across MGs without an operator action
and without a release.

## CPU capacity

The MG measures its CPU capacity as a score of a fixed benchmark, in
**benchmark iterations per second**. One iteration is one second of a
10-guardian seat's consensus at the production round delay: 20 rounds that
each consensus-encode, hash and Schnorr-sign one unit and verify one peer
signature, plus every 100th iteration a five-note mint-v2 reissue (5 blind
signatures, 5 note verifications and 5 Schnorr verifications). It is
CPU-only. The seat cost simulation's benchmark also exchanges the units over
Iroh and commits them to RocksDB, but the readiness pressure check already
gates the disk, and a startup benchmark must not open endpoints or write
files.

The capacity is the score summed over one worker per CPU the MG may use
(`available_parallelism`, which follows the affinity mask and the cgroup CPU
quota): the median of three 3-second runs after a 1-second warm-up.
Simultaneous multithreading and a busy host lower it, which is what a seat
would meet. A 3-second run of one worker follows; its score tells per-core
speed apart from core count and is reported, not used. The benchmark takes
about 13 seconds. On Fedi's devbox (an AMD EPYC 9454P with 96 CPUs, shared
and busy) it scored 25,000 to 40,000 iterations per second, and 790 to 820
for one worker.

An iteration means the work of one benchmark version (`BENCHMARK_VERSION`,
now 1), and so does the publication's `seat_cpu_iterations_per_sec`. A change
that moves the score, such as to the benchmark's work, its constants, its
crypto dependencies or the compiler settings, needs a new benchmark version.
That version also needs a new policy field (`seat_cpu_iterations_per_sec_v2`)
that only MGs of the new version read, because its budget is in other units;
MGs of the old version keep reading the old field.

The MG measures once, at daemon start before its seats start, when it has no
measurement. It stores the result with the benchmark version, the CPU count
and the CPU model (the first `model name` in `/proc/cpuinfo`, empty where
there is none) it was taken at, and measures again only when a later start
sees a different version, count or model, for example after an MG upgrade
that changes the benchmark, a move to new hardware or a changed CPU quota. A
measurement on a busy host stays until then. A measurement that fails, for
example because a worker thread cannot be spawned, logs a warning and leaves
the MG without a CPU limit until a later start measures; it does not stop the
MG. Local E2E starts many
MGs on one host and skips the benchmark, so it has no CPU limit, unless a
test turns it on for one MG with `MG_E2E_CPU_BENCHMARK`.

`seat_cpu_iterations_per_sec` has no default, unlike the memory budget:
there are no fleet figures yet, and a guessed default could cut seats on
every host. MGs journal their scores and CPU limit in the `seat capacity`
event, and Fedi publishes a budget once that telemetry shows what seats
need. The bounds, 1 to 100,000, refuse a budget below one iteration per
second, the least a seat's own consensus can need, and one so large it would
stop every MG.

When the runtime limits shipped, a migration made the operator maximum
automatic where it was from 1 to 8. The onboarding form had pre-filled a RAM
recommendation capped at 8, so such a value most likely was that
recommendation, not an operator choice. Values above 8 and 0 (not selling)
stay. The replaced value stays in `offer_state.pre_automatic_max_seats`, so it
can be restored. The migration replaces the offer epoch, because the first
memory value of 4 can take the last free slot.

The MG writes the runtime limits and the publication's inputs at start, on
each newly admitted publication, and every 5 minutes. The CPU and disk
limits change only with the publication. The memory limit falls at once and rises
only after three consecutive samples allow it, so a transient spike does not
open and close the offer. A runtime or data limit below the active seat count
only stops new sales. It does not stop a seat. Each write keeps the offer-epoch
invariants of [SPEC-quote-settlement](./SPEC-quote-settlement.md).

## Disk

Every seat syncs its consensus database many times a second, so seats wear
the drive that holds them. Drive wear, not space, is what bounds seats on
disk: a seat's database stays small while its writes add up to many times the
drive's size over its life. The MG cannot read a drive's rated endurance
(TBW) or its wear: containers have no device access, and ratings vary widely
between drives of one size. So it uses the size of the data filesystem as
the proxy, since endurance grows with capacity, and sells one seat per
`seat_disk_gb` of it. An MG acts as a hosting provider here: the figure sets
how hard the fleet may work a drive, and the host chooses hardware for that.

The size is the filesystem's total size, used or not (`statvfs` blocks times
fragment size, what `df` shows as its size), not its free space. GB means
10^9 bytes, as drives are sold. The default of 80 GB sells 6 seats on a
512 GB drive and 12 on a 1 TB drive, whose ext4 filesystems report about 503
and 984 GB after the filesystem's own overhead. On a pool spanning several
drives, or a data directory on a shared host filesystem, the MG sees that
filesystem's size, which is still the drive capacity behind its seats.

The MG measures once, at daemon start. A failed measurement logs a warning
and leaves the MG without a disk limit; it does not stop the MG. Local E2E
starts many MGs on one host's filesystem, often a small temporary one, and
skips the measurement, so it has no disk limit. MGs journal the disk limit in
the `seat capacity` event and the filesystem's kind and size in the `cgroup
resource sample`, so Fedi can tune `seat_disk_gb` without a release. The
bounds, 1 to 100,000 GB, refuse a figure that would let a small drive sell
seats that cost almost nothing, and one that would stop every MG.

The figure does not model writes per seat. Fedi measures those across the
fleet (`cgroup_block_write_bytes` in the resource sample) and moves the
figure when they change, for example when seats sync less often.

## Data budget

Some hosts run on home internet with a monthly data cap that the household
shares. The MG cannot know how much of that cap the operator wants to give
it, so the operator states it. The answer is one of three values, stored
with the operator maximum and the price:

- **not set**: no answer. Every MG set up before the question existed
  starts here, and so applies no data limit. The dashboard asks the
  operator to answer, without a warning: nothing is wrong;
- **unlimited**: the host's internet has no monthly data cap. No data
  limit applies;
- **a budget** in GB per 30 days, from 1.

Setup requires unlimited or a budget, and an operator can change between
them later but cannot go back to not set. An operator who does not know is
pointed to their internet plan or bill, not to an answer: a host with a cap
that answers unlimited could use up the household's cap. Unlimited and not set limit seats
alike; they are separate so that the `seat capacity` telemetry event can tell
a host without a cap from one whose operator never answered.

The MG does not estimate what a seat costs from its own seats. The figure is
the publication's `seat_monthly_data_gb`
([SPEC-setup-payment-federations](../../../specs/SPEC-setup-payment-federations.md)),
and like the memory budget it has a built-in default: 65 GB per seat a month.
Fedi can move it from its fleet-wide view of the guardians'
`fm_peer_transport_bytes_total` without a release. So a budget limits seats
from the release that brings it, without waiting for a publication. This
replaces an earlier decision that an absent figure applied no data limit: a
host that gave a budget would then sell without one until Fedi published.

A budget below one seat's figure, such as 50 GB at 65 GB a seat, covers no
seat. It is accepted, because it is the operator's honest answer, and the
MG then sells no new seat. The dashboard and the CLI say so when it is set,
and do not call the host full, since it may run no seat at all.

The budget is a sales limit, not a meter. Running seats keep running, and a
busy federation can use more than the figure. The MG does not count
month-to-date use against the budget, so it does not stop selling when a
month's use runs high.

Across production seats a guardian's peer traffic averaged about 22 KB/s,
sent plus received: about 57 GB per 30 days on nearly idle federations. It
grows about 2 KB/s with each peer, from about 14 KB/s with 7 peers to about
26 KB/s with 12. The default of 65 GB sits a little above the idle average,
so it suits typical federation sizes and underestimates large or busy ones.

## Measurement basis

A staging measurement on Umbrel with one sold seat on a young Signet federation after about ten hours reported 171 MB RSS for
the daemon (embedded operator UI, Iroh, Nostr, setup-payment wallet client)
plus 281 MB RSS for the seat's `fedimintd`. The original 1.5 GB/seat budget was ~5×
that resting per-seat cost: headroom for DKG and chain-scan spikes and for
`fedimintd` growth as a federation accumulates history.

The policy limit (default 12) bounds what RAM does not: concurrent-ceremony
CPU and disk IO, for which no evidence exists yet. The disk limit bounds
drive wear, not IO contention. The readiness pressure
check ([SPEC-advertisement](./SPEC-advertisement.md#readiness-gate)) stops
new sales on a host whose seats keep waiting for CPU or disk, at maximums
Fedi can publish and that otherwise default; every MG journals its pressure
averages, which is the evidence for tuning those maximums and, later, for
relaxing the policy limit. Re-measure once seats
carry weeks of mainnet-like history before treating the per-seat budget as
settled; the young-federation measurement is the weakest part of this
basis.

The budget moved from 1.5 GiB to 1.2 GiB as a conservative first step after
a 7-guardian federation under a 30-minute payment and ecash workload measured
`fedimintd` at 129 MiB peak RSS per seat with a bounded blocking pool. The
publication can move it further without a release.

Peak RSS depends on the host's transparent huge page mode. With
`transparent_hugepage=always`, a local 10-guardian federation with production
session length, client load and restarts peaked over 700 MiB per seat after 4
hours, and climbing, with most anonymous memory in huge pages. The same load
with huge pages off peaked near 225 MiB. So the daemon turns huge pages off for
itself and its seats at start, and a budget measured before that change
overstates what a seat needs.
