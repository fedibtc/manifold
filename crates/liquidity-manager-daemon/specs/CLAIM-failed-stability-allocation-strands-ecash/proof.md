# Current argument

## Argument

Current code commits a caller-owned deposit identity and tuple before submission,
reuses it after restart, drains the terminal operation stream, and leaves short
balance or rejected-deposit cases `action_required` for authenticated inspection
and case-dependent recovery or audited abandonment. Completed funding can block
retry, and an existing operation id can block replacement binding. The historical automatic no-ID stranding
trace therefore no longer executes.

## Unresolved proof obligations

The universal claim still requires a complete enumeration of every post-claimed
automatic terminal writer and every recovery transition. The corrupt-step
abandonment bypass is outside the faithful-store assumption unless a current
production writer can create that state. No current in-envelope automatic
stranding counterexample has been established.

## Weakest links

The production exit/writer enumeration and later configuration-change behavior
remain incomplete.
