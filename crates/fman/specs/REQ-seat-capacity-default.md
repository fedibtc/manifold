# REQ-seat-capacity-default: Recommend a seat count the host's RAM can carry

Source: FMan operator sizing and the measured per-seat memory budget.

The FMan recommends an initial durable seat capacity from **available** RAM in
the onboarding offer form. It is a recommendation the operator may override,
not an enforced cap, since other services share the host. Capacity is no
longer a daemon-start argument.

The normative rule: one seat per whole 1.5 GiB of available RAM, capped at 8
seats. GB here means GiB (2³⁰ bytes, the kernel's binary units); fractional
budget truncates toward fewer seats, and a host with less than 1.5 GiB
available is recommended 0 seats — too small to sell any.

| Available RAM | Recommended seats |
| --- | --- |
| < 1.5 GB | 0 |
| 1.5 GB | 1 |
| 3.0 GB | 2 |
| 4.5 GB | 3 |
| 6.0 GB | 4 |
| 7.5 GB | 5 |
| 9.0 GB | 6 |
| 10.5 GB | 7 |
| ≥ 12 GB | 8 |

## Measurement basis

A staging measurement on Umbrel with one sold seat on a young Signet federation after about ten hours reported 171 MB RSS for
the daemon (embedded operator UI, Iroh, Nostr, setup-payment wallet client)
plus 281 MB RSS for the seat's `fedimintd`. The 1.5 GB/seat budget is ~5×
that resting per-seat cost: headroom for DKG and chain-scan spikes and for
`fedimintd` growth as a federation accumulates history.

The 8-seat cap reflects that beyond it the expected binding constraints are
concurrent-ceremony CPU and disk IO, for which no evidence exists yet, not
RAM. Re-measure once seats carry weeks of mainnet-like history before
treating either the per-seat budget or the cap as settled; the
young-federation measurement is the weakest part of this basis.
