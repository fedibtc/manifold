# Operation can terminalize between its fence and send

At source change `moqkvurn`, an allocation worker commits the
`Pending`-to-`InDoubt` funding-submission compare-and-set. Delay delivery or
polling of that successful result. Before the worker invokes `send_onchain`, an
ordinary third-party output paying the operation's exact address and amount can
be confirmed and exclusively claimed by normal chain reconciliation, which
commits the operation as `completed`.

The older invocation then resumes from its already successful fence result and
calls `send_onchain` without another durable state check. This violates the
claim's literal prohibition on a later irreversible effect after terminal
status. The immediate scheduling, storage, observation, and external-effect
assumptions all permit the trace. This is a source-derived counterexample; no
focused runtime reproducer was added.
