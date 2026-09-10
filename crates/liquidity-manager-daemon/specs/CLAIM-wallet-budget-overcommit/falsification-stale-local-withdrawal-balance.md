# Withdrawal mixes an old balance with a newer reconciliation watermark

At source change `moqkvurn`, let withdrawal A read a high spendable balance and
delay its response. Another withdrawal then spends value, settles, and persists
a later lower wallet observation whose read tick follows that settlement.
When A resumes, the monotonic observation upsert correctly rejects A's older
read, but A retains the older balance in its Rust value.

Withdrawal admission opens its serialized transaction and computes outgoing
accounting from the newer durable watermark while passing the older local
spendable balance as `B`. It can therefore release the settled debit from
outgoing exposure and commit A even when A plus the fee reserve exceeds the
latest durable balance. Every balance observation was truthful when read and
the immediate assumptions remain granted. This is a source-derived
counterexample; no focused runtime reproducer was added.
