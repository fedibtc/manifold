# Current argument

## Argument

The current submission compare-and-set prevents a worker that loses the fence
from performing an irreversible effect. It does not reserve exclusivity through
the later external call: normal reconciliation can terminalize the operation
after the CAS commits but before the winning invocation observes the result and
calls `send_onchain`. The current counterexample is recorded in
[`falsification-terminal-between-fence-and-send.md`](falsification-terminal-between-fence-and-send.md).

## Weakest links

A local predecessor CAS is not a lease or transaction spanning the external
effect. Repair needs a mechanism that prevents terminalization in this interval
or a differently authorized semantic property.
