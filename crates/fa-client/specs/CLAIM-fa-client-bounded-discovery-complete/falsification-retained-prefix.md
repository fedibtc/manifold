# Falsification: Bounded discovery retained prefix

`candidate_cap_can_omit_an_honest_eligible_advertisement` supplies 2,048
distinct, valid, fresh, eligible advertisements followed by an equally eligible
honest advertisement. The retained-prefix candidate limit excludes the honest
event. The 16 MiB bound has the same completion semantics, although this
counterexample uses the candidate limit.

This does not assert relay withholding or equivocation, and it does not concern
trust verification after an advertisement has been retained.
