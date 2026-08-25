# Proof: Selection deadline boundary

`cheap_slow_spam_times_out_before_honest_badge_verification` records the
adversarial ordering: slow, cheaper invalid advertisements consume the whole
preview budget before honest verification begins. The expected result is
`SelectionPreviewTimeout`, not `InsufficientFmanSeats`. The timer wraps
discovery and the complete selection walk; expiry wins an exact tie, while a
completed walk that exhausts its pool strictly before expiry reports the
shortfall.

The timeout does not guarantee that an honest retained candidate was reached or
that an honest advertisement was retained. Bounded discovery can omit one, as
recorded by
[CLAIM-fi-client-bounded-discovery-complete](../CLAIM-fi-client-bounded-discovery-complete.md).

The parent claim remains `Unverified`; this file records the current evidence
and boundary without a verdict.
