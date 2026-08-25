# Falsification: Mint v1 consolidation spends pre existing operator principal

Fleet Manager hands accepted mint-v1 notes to the same Fedimint client scope that
holds the operator's other notes. The primary finalizer may consolidate those
older notes into the reissue and charge their input fees. The claim can complete
successfully even when the resulting ordinary-wallet balance is lower than it was
before the customer payment.

The executable transcription in
[`model_mint_v1_wallet.py`](../model_mint_v1_wallet.py) demonstrates the current
counterexample:

```console
$ python3 crates/fman/specs/model_mint_v1_wallet.py mixed \
    --gross-sat 192 --tier-msat 67108864 --notes 9
claim 1: delta=-486.386 sat, cost=678.386 sat, consolidated=5
```

The FI contributes 192 sat, but consolidating five of nine older notes in the
selected tier reduces pre-existing operator principal by 486.386 sat. No
malformed payment or malicious counterparty is required.

The Python model transcribes the pinned mint-v1 fee, consolidation, amount
representation, and primary-module accounting paths. Its correspondence to the
dependency-owned Rust finalizer is the weakest evidence step. Mint-v2 and future
dependency behavior are outside this counterexample.
