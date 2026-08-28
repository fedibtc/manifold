#!/usr/bin/env python3
"""Model the pinned Fedimint mint-v1 finalizer used by paid-seat claims.

This is an executable transcription of the fee and note-shaping paths in
Fedimint release v0.11.1-fedi16 (commit
881b0c2eda6b4b97785fce977a9c7ea65942a0ee):

* ``FeeConsensus`` in ``modules/fedimint-mint-common/src/config.rs``;
* ``consolidate_notes`` and ``represent_amount`` in
  ``modules/fedimint-mint-client/src/lib.rs``; and
* the primary-module accounting in ``fedimint-client/src/client.rs``;
* lnv2 federation fees and send/refund transactions in
  ``modules/fedimint-lnv2-{common,client}``; and
* the default gateway allowance in ``modules/fedimint-lnv2-common``.

It models successful external-note reissues (the accepted mint-v1 claim),
sequential Lightning spend/refund attempts, and a bounded set of completed
claim/Lightning interleavings. Its important accounting result is the change
in ordinary wallet balance attributable to a newly claimed payment;
previously held note principal is not counted as new seat revenue.
"""

from __future__ import annotations

import argparse
import dataclasses
import sys
import unittest
from collections.abc import Iterable


MSATS_PER_BTC = 100_000_000_000
MAX_DENOMINATION_MSATS = 1_000_000 * MSATS_PER_BTC
MAX_U64 = 2**64 - 1

MINT_FEE_BASE_MSATS = 100
MINT_FEE_PARTS_PER_MILLION = 1_000
LIGHTNING_FEE_BASE_MSATS = 1_000
LIGHTNING_FEE_PARTS_PER_MILLION = 1_000
GATEWAY_FEE_BASE_MSATS = 4_000
GATEWAY_FEE_PARTS_PER_MILLION = 6_000

MAX_NOTES_PER_TIER_TRIGGER = 8
MIN_NOTES_PER_TIER = 4
MAX_NOTES_TO_CONSOLIDATE = 20
TARGET_NOTES_PER_TIER = 2


def checked_u64(value: int, label: str) -> int:
    if not 0 <= value <= MAX_U64:
        raise OverflowError(f"{label} is outside u64: {value}")
    return value


def sum_u64(values: Iterable[int], label: str) -> int:
    total = 0
    for value in values:
        total = checked_u64(total + value, label)
    return total


def mint_fee_msats(amount_msats: int) -> int:
    """Maximum permitted mint-v1 fee for one input or output."""
    amount_msats = checked_u64(amount_msats, "fee input")
    product = min(MAX_U64, amount_msats * MINT_FEE_PARTS_PER_MILLION)
    relative = product // 1_000_000
    return checked_u64(MINT_FEE_BASE_MSATS + relative, "mint fee")


def proportional_fee_msats(amount_msats: int, base_msats: int, ppm: int) -> int:
    amount_msats = checked_u64(amount_msats, "fee input")
    product = min(MAX_U64, amount_msats * ppm)
    relative = product // 1_000_000
    return checked_u64(base_msats + relative, "proportional fee")


def lightning_fee_msats(amount_msats: int) -> int:
    """Maximum permitted lnv2 federation fee for one input or output."""
    return proportional_fee_msats(
        amount_msats, LIGHTNING_FEE_BASE_MSATS, LIGHTNING_FEE_PARTS_PER_MILLION
    )


def gateway_fee_msats(invoice_msats: int) -> int:
    """The policy's default gateway send allowance."""
    return proportional_fee_msats(
        invoice_msats, GATEWAY_FEE_BASE_MSATS, GATEWAY_FEE_PARTS_PER_MILLION
    )


def standard_tiers() -> tuple[int, ...]:
    """The pinned mint-v1 server's powers-of-two denomination set."""
    tiers: list[int] = []
    denomination = 1
    while denomination <= MAX_DENOMINATION_MSATS:
        tiers.append(denomination)
        denomination *= 2
    return tuple(tiers)


STANDARD_TIERS = standard_tiers()


def quote_denominations(amount_msats: int, tiers: tuple[int, ...]) -> list[int]:
    """Mirror FMan's greedy ``quote_denominations`` payment breakdown."""
    remaining = checked_u64(amount_msats, "quoted amount")
    selected: list[int] = []
    for tier in reversed(tiers):
        while tier <= remaining:
            selected.append(tier)
            remaining -= tier
    if remaining:
        raise ValueError(f"{amount_msats} msat is not representable")
    return selected


def represented_outputs(
    amount_msats: int,
    current_counts: dict[int, int],
    tiers: tuple[int, ...],
) -> tuple[dict[int, int], int]:
    """Mirror ``represent_amount(..., denomination_sets=2, ...)``.

    Returned ``dust_msats`` is the transaction overfunding left after output
    value and output fees.  Fedimint asserts it is no greater than the fee for
    a one-msat output.
    """
    remaining = checked_u64(amount_msats, "amount to represent")
    outputs: dict[int, int] = {}

    for tier in tiers:
        missing = max(0, TARGET_NOTES_PER_TIER - current_counts.get(tier, 0))
        cost = checked_u64(tier + mint_fee_msats(tier), "output cost")
        count = min(remaining // cost, missing)
        if count:
            outputs[tier] = outputs.get(tier, 0) + count
            remaining -= cost * count

    for tier in reversed(tiers):
        cost = checked_u64(tier + mint_fee_msats(tier), "output cost")
        count = remaining // cost
        if count:
            outputs[tier] = outputs.get(tier, 0) + count
            remaining -= cost * count

    if remaining > mint_fee_msats(1):
        raise AssertionError(
            f"Fedimint representation invariant failed: {remaining} msat remains"
        )
    return outputs, remaining


@dataclasses.dataclass(frozen=True)
class ClaimResult:
    payment_msats: int
    balance_before_msats: int
    balance_after_msats: int
    payment_input_count: int
    payment_input_fees_msats: int
    consolidation_input_count: int
    consolidation_input_value_msats: int
    consolidation_input_fees_msats: int
    output_count: int
    output_value_msats: int
    output_fees_msats: int
    dust_msats: int

    @property
    def incremental_balance_msats(self) -> int:
        return self.balance_after_msats - self.balance_before_msats

    @property
    def total_cost_msats(self) -> int:
        return self.payment_msats - self.incremental_balance_msats


@dataclasses.dataclass(frozen=True)
class FinalizerResult:
    balance_before_msats: int
    balance_after_msats: int
    partial_input_msats: int
    partial_output_msats: int
    consolidation_input_count: int
    consolidation_input_value_msats: int
    consolidation_input_fees_msats: int
    funding_input_count: int
    funding_input_value_msats: int
    funding_input_fees_msats: int
    change_output_count: int
    change_output_value_msats: int
    change_output_fees_msats: int
    dust_msats: int

    @property
    def wallet_delta_msats(self) -> int:
        return self.balance_after_msats - self.balance_before_msats

    @property
    def mint_churn_cost_msats(self) -> int:
        return (
            self.consolidation_input_fees_msats
            + self.funding_input_fees_msats
            + self.change_output_fees_msats
            + self.dust_msats
        )


@dataclasses.dataclass(frozen=True)
class LightningAttemptResult:
    succeeded: bool
    invoice_msats: int
    gateway_fee_msats: int
    contract_msats: int
    funding: FinalizerResult
    refund: FinalizerResult | None

    @property
    def wallet_cost_msats(self) -> int:
        after = (
            self.refund.balance_after_msats
            if self.refund is not None
            else self.funding.balance_after_msats
        )
        return self.funding.balance_before_msats - after


@dataclasses.dataclass(frozen=True)
class FundedLightning:
    invoice_msats: int
    gateway_fee_msats: int
    contract_msats: int
    funding: FinalizerResult


@dataclasses.dataclass(frozen=True)
class SequentialSettlementResult:
    gross_msats: int
    claim: ClaimResult
    failed_attempts: tuple[LightningAttemptResult, ...]
    success: LightningAttemptResult

    @property
    def delivered_msats(self) -> int:
        return self.success.invoice_msats


class MintV1WalletModel:
    def __init__(
        self,
        counts: dict[int, int] | None = None,
        tiers: tuple[int, ...] = STANDARD_TIERS,
    ) -> None:
        self.tiers = tiers
        self.counts = {
            tier: count for tier, count in (counts or {}).items() if count > 0
        }
        unknown = set(self.counts).difference(tiers)
        if unknown:
            raise ValueError(f"wallet contains unsupported tiers: {sorted(unknown)}")

    @property
    def balance_msats(self) -> int:
        return sum_u64(
            (tier * count for tier, count in self.counts.items()), "wallet balance"
        )

    def copy(self) -> MintV1WalletModel:
        return MintV1WalletModel(self.counts.copy(), self.tiers)

    def _take_consolidation_inputs(self) -> list[int]:
        if not any(
            count > MAX_NOTES_PER_TIER_TRIGGER for count in self.counts.values()
        ):
            return []

        remaining = MAX_NOTES_TO_CONSOLIDATE
        selected: list[int] = []
        for tier in sorted(self.counts):
            take = min(max(0, self.counts[tier] - MIN_NOTES_PER_TIER), remaining)
            if take:
                selected.extend([tier] * take)
                self.counts[tier] -= take
                if self.counts[tier] == 0:
                    del self.counts[tier]
                remaining -= take
        return selected

    def _select_funding_inputs(self, requested_msats: int) -> list[int]:
        """Mirror mint-v1 ``SelectNotesWithAtleastAmount``."""
        if requested_msats == 0:
            return []

        notes = [
            tier
            for tier in sorted(self.counts, reverse=True)
            for _ in range(self.counts[tier])
        ]
        selected: list[int] = []
        # (amount, insertion checkpoint)
        last_big_note: tuple[int, int] | None = None
        pending = requested_msats

        for note in notes:
            note_fee = mint_fee_msats(note)
            if note <= note_fee:
                continue
            needed_with_fee = checked_u64(pending + note_fee, "funding target")
            if note < needed_with_fee:
                pending = checked_u64(pending + note_fee - note, "pending funding")
                selected.append(note)
            elif note > needed_with_fee:
                last_big_note = (note, len(selected))
            else:
                selected.append(note)
                break
        else:
            if last_big_note is None:
                raise ValueError("insufficient economical mint-v1 balance")
            note, checkpoint = last_big_note
            del selected[checkpoint:]
            selected.append(note)

        selected_value = sum_u64(selected, "selected funding value")
        selected_fees = sum_u64(
            (mint_fee_msats(note) for note in selected), "selected funding fees"
        )
        if selected_value < requested_msats + selected_fees:
            raise AssertionError("funding selector returned insufficient notes")
        for note in selected:
            count = self.counts.get(note, 0)
            if count == 0:
                raise AssertionError("funding selector reused a note")
            if count == 1:
                del self.counts[note]
            else:
                self.counts[note] = count - 1
        return selected

    def _finalize(
        self,
        *,
        partial_input_msats: int,
        partial_output_msats: int,
    ) -> FinalizerResult:
        """Run the generic primary mint-v1 finalizer for one transaction.

        The partial output amount already includes every fee charged by the
        transaction's non-primary inputs and outputs, matching
        ``transaction_builder_get_balance``.
        """
        balance_before = self.balance_msats
        input_amount = checked_u64(partial_input_msats, "partial inputs")
        output_amount = checked_u64(partial_output_msats, "partial outputs and fees")

        consolidation_inputs = self._take_consolidation_inputs()
        consolidation_value = sum_u64(
            consolidation_inputs, "consolidation input value"
        )
        consolidation_fees = sum_u64(
            (mint_fee_msats(tier) for tier in consolidation_inputs),
            "consolidation input fees",
        )
        input_amount = checked_u64(
            input_amount + consolidation_value, "inputs after consolidation"
        )
        output_amount = checked_u64(
            output_amount + consolidation_fees, "outputs after consolidation fees"
        )

        funding_inputs = self._select_funding_inputs(max(0, output_amount - input_amount))
        funding_value = sum_u64(funding_inputs, "funding input value")
        funding_fees = sum_u64(
            (mint_fee_msats(tier) for tier in funding_inputs), "funding input fees"
        )
        input_amount = checked_u64(input_amount + funding_value, "total inputs")
        output_amount = checked_u64(output_amount + funding_fees, "outputs and input fees")
        if input_amount < output_amount:
            raise AssertionError("primary mint-v1 finalizer underfunded transaction")

        outputs, dust = represented_outputs(
            input_amount - output_amount, self.counts, self.tiers
        )
        output_value = sum_u64(
            (tier * count for tier, count in outputs.items()), "change output value"
        )
        output_fees = sum_u64(
            (mint_fee_msats(tier) * count for tier, count in outputs.items()),
            "change output fees",
        )
        if input_amount != output_amount + output_value + output_fees + dust:
            raise AssertionError("modeled finalized transaction does not balance")
        for tier, count in outputs.items():
            self.counts[tier] = self.counts.get(tier, 0) + count

        return FinalizerResult(
            balance_before_msats=balance_before,
            balance_after_msats=self.balance_msats,
            partial_input_msats=partial_input_msats,
            partial_output_msats=partial_output_msats,
            consolidation_input_count=len(consolidation_inputs),
            consolidation_input_value_msats=consolidation_value,
            consolidation_input_fees_msats=consolidation_fees,
            funding_input_count=len(funding_inputs),
            funding_input_value_msats=funding_value,
            funding_input_fees_msats=funding_fees,
            change_output_count=sum(outputs.values()),
            change_output_value_msats=output_value,
            change_output_fees_msats=output_fees,
            dust_msats=dust,
        )

    def claim(self, payment_msats: int) -> ClaimResult:
        """Apply one successful accepted mint-v1 claim to the wallet state."""
        payment_inputs = quote_denominations(payment_msats, self.tiers)
        payment_fees = sum_u64(
            (mint_fee_msats(tier) for tier in payment_inputs), "payment input fees"
        )
        finalized = self._finalize(
            partial_input_msats=payment_msats, partial_output_msats=payment_fees
        )
        result = ClaimResult(
            payment_msats=payment_msats,
            balance_before_msats=finalized.balance_before_msats,
            balance_after_msats=finalized.balance_after_msats,
            payment_input_count=len(payment_inputs),
            payment_input_fees_msats=payment_fees,
            consolidation_input_count=finalized.consolidation_input_count,
            consolidation_input_value_msats=finalized.consolidation_input_value_msats,
            consolidation_input_fees_msats=finalized.consolidation_input_fees_msats,
            output_count=finalized.change_output_count,
            output_value_msats=finalized.change_output_value_msats,
            output_fees_msats=finalized.change_output_fees_msats,
            dust_msats=finalized.dust_msats,
        )
        expected_delta = (
            payment_msats
            - payment_fees
            - finalized.consolidation_input_fees_msats
            - finalized.funding_input_fees_msats
            - finalized.change_output_fees_msats
            - finalized.dust_msats
        )
        if result.incremental_balance_msats != expected_delta:
            raise AssertionError("wallet delta does not match transaction costs")
        return result

    def _fund_lightning(self, invoice_msats: int) -> tuple[FinalizerResult, int, int]:
        gateway_fee = gateway_fee_msats(invoice_msats)
        contract = checked_u64(invoice_msats + gateway_fee, "outgoing contract")
        contract_output_fee = lightning_fee_msats(contract)
        finalized = self._finalize(
            partial_input_msats=0,
            partial_output_msats=checked_u64(
                contract + contract_output_fee, "lightning output and fee"
            ),
        )
        return finalized, gateway_fee, contract

    def max_send_invoice_msats(self) -> int:
        """Largest invoice the current wallet can fund with policy fees."""
        low = 0
        high = self.balance_msats
        while low < high:
            candidate = (low + high + 1) // 2
            try:
                self.copy()._fund_lightning(candidate)
            except ValueError:
                high = candidate - 1
            else:
                low = candidate
        return low

    def lightning_attempt(self, *, succeed: bool) -> LightningAttemptResult:
        """Fund a send-all lnv2 payment and optionally apply its refund."""
        funded = self.fund_send_all()
        refund = None if succeed else self.refund(funded)
        return LightningAttemptResult(
            succeeded=succeed,
            invoice_msats=funded.invoice_msats,
            gateway_fee_msats=funded.gateway_fee_msats,
            contract_msats=funded.contract_msats,
            funding=funded.funding,
            refund=refund,
        )

    def fund_send_all(self) -> FundedLightning:
        """Construct and fund one maximum-value lnv2 send."""
        invoice = self.max_send_invoice_msats()
        if invoice == 0:
            raise ValueError("wallet cannot fund a non-zero Lightning invoice")
        funding, gateway_fee, contract = self._fund_lightning(invoice)
        return FundedLightning(
            invoice_msats=invoice,
            gateway_fee_msats=gateway_fee,
            contract_msats=contract,
            funding=funding,
        )

    def refund(self, funded: FundedLightning) -> FinalizerResult:
        """Refund one previously funded failed outgoing contract."""
        return self._finalize(
            partial_input_msats=funded.contract_msats,
            partial_output_msats=lightning_fee_msats(funded.contract_msats),
        )


def sequential_settlement(
    gross_msats: int, failed_attempts: int = 4
) -> SequentialSettlementResult:
    """Claim into an empty wallet, refund failures, then settle successfully."""
    wallet = MintV1WalletModel()
    claim = wallet.claim(gross_msats)
    failed = tuple(
        wallet.lightning_attempt(succeed=False) for _ in range(failed_attempts)
    )
    success = wallet.lightning_attempt(succeed=True)
    return SequentialSettlementResult(gross_msats, claim, failed, success)


def sequential_gross_price(
    target_msats: int,
    failed_attempts: int = 4,
    grid_msats: int = 512,
) -> int:
    """Smallest grid-aligned gross for the sequential empty-wallet baseline."""
    if target_msats <= 0:
        raise ValueError("net target must be positive")
    if grid_msats <= 0:
        raise ValueError("price grid must be positive")
    low = 1
    high = max(1, (target_msats + grid_msats - 1) // grid_msats)
    while (
        sequential_settlement(high * grid_msats, failed_attempts).delivered_msats
        < target_msats
    ):
        high *= 2
    while low < high:
        middle = (low + high) // 2
        delivered = sequential_settlement(
            middle * grid_msats, failed_attempts
        ).delivered_msats
        if delivered >= target_msats:
            high = middle
        else:
            low = middle + 1
    return low * grid_msats


@dataclasses.dataclass(frozen=True)
class InterleavingResult:
    delivered_msats: int
    remaining_balance_msats: int
    schedule: tuple[str, ...]


def enumerate_completed_claim_interleavings(
    gross_msats: int,
    claims: int,
    failed_attempts: int = 4,
) -> tuple[InterleavingResult, InterleavingResult, int]:
    """Enumerate completed claims around one-at-a-time lnv2 attempts.

    Claim transactions may complete before, during, or after each funded
    failed attempt. Lightning attempts do not overlap each other. The final
    successful send starts after every claim and failed refund completes.
    Returns minimum delivery, maximum delivery, and schedule count.

    This intentionally does not yet split transaction construction from mint
    output finalization. It is a useful concurrent-operation envelope, not the
    final exhaustive scheduler promised by the spec.
    """
    if claims <= 0:
        raise ValueError("at least one claim is required")

    outcomes: list[InterleavingResult] = []

    def visit(
        wallet: MintV1WalletModel,
        claims_done: int,
        failures_done: int,
        in_flight: FundedLightning | None,
        schedule: tuple[str, ...],
    ) -> None:
        if in_flight is None and failures_done == failed_attempts and claims_done == claims:
            final_wallet = wallet.copy()
            success = final_wallet.fund_send_all()
            outcomes.append(
                InterleavingResult(
                    success.invoice_msats,
                    final_wallet.balance_msats,
                    schedule + ("success",),
                )
            )
            return

        if claims_done < claims:
            claimed = wallet.copy()
            claimed.claim(gross_msats)
            visit(
                claimed,
                claims_done + 1,
                failures_done,
                in_flight,
                schedule + (f"claim-{claims_done + 1}",),
            )

        if in_flight is not None:
            refunded = wallet.copy()
            refunded.refund(in_flight)
            visit(
                refunded,
                claims_done,
                failures_done + 1,
                None,
                schedule + (f"refund-{failures_done + 1}",),
            )
        elif failures_done < failed_attempts and wallet.balance_msats:
            funded_wallet = wallet.copy()
            try:
                funded = funded_wallet.fund_send_all()
            except ValueError:
                return
            visit(
                funded_wallet,
                claims_done,
                failures_done,
                funded,
                schedule + (f"fund-failure-{failures_done + 1}",),
            )

    visit(MintV1WalletModel(), 0, 0, None, ())
    if not outcomes:
        raise ValueError("no complete settlement schedule is fundable")
    return (
        min(outcomes, key=lambda result: result.delivered_msats),
        max(outcomes, key=lambda result: result.delivered_msats),
        len(outcomes),
    )


def format_msats(msats: int) -> str:
    return f"{msats / 1_000:,.3f} sat"


def print_result(index: int, result: ClaimResult) -> None:
    print(
        f"claim {index}: delta={format_msats(result.incremental_balance_msats)}, "
        f"cost={format_msats(result.total_cost_msats)}, "
        f"consolidated={result.consolidation_input_count} "
        f"({format_msats(result.consolidation_input_value_msats)}), "
        f"fees=input {format_msats(result.payment_input_fees_msats)}, "
        f"consolidation {format_msats(result.consolidation_input_fees_msats)}, "
        f"output {format_msats(result.output_fees_msats)}, "
        f"dust {format_msats(result.dust_msats)}"
    )


def print_attempt(index: int, result: LightningAttemptResult) -> None:
    outcome = "success" if result.succeeded else "refunded"
    print(
        f"attempt {index}: {outcome}, invoice={format_msats(result.invoice_msats)}, "
        f"gateway allowance={format_msats(result.gateway_fee_msats)}, "
        f"wallet cost={format_msats(result.wallet_cost_msats)}, "
        f"funding inputs={result.funding.funding_input_count}, "
        f"funding consolidation={result.funding.consolidation_input_count}, "
        f"refund consolidation="
        f"{result.refund.consolidation_input_count if result.refund else 0}"
    )


class ModelTests(unittest.TestCase):
    def test_empty_wallet_claim_balances(self) -> None:
        result = MintV1WalletModel().claim(192_000)
        self.assertEqual(result.incremental_balance_msats, 187_022)
        self.assertEqual(result.total_cost_msats, 4_978)
        self.assertEqual(result.consolidation_input_count, 0)

    def test_repeated_same_price_claims_have_small_consolidation_cost(self) -> None:
        wallet = MintV1WalletModel()
        results = [wallet.claim(192_000) for _ in range(15)]
        worst = min(results, key=lambda result: result.incremental_balance_msats)
        self.assertEqual(worst.total_cost_msats, 4_978)
        self.assertTrue(any(result.consolidation_input_count for result in results))

    def test_high_value_prior_notes_can_consume_more_than_payment_markup(self) -> None:
        tier = 2**26
        result = MintV1WalletModel({tier: 9}).claim(192_000)
        self.assertEqual(result.consolidation_input_count, 5)
        self.assertEqual(result.consolidation_input_fees_msats, 336_040)
        self.assertEqual(result.output_fees_msats, 341_442)
        self.assertEqual(result.incremental_balance_msats, -486_386)

    def test_failed_attempt_refunds_gateway_allowance_but_loses_churn_fees(self) -> None:
        wallet = MintV1WalletModel()
        wallet.claim(192_000)
        before = wallet.balance_msats
        attempt = wallet.lightning_attempt(succeed=False)
        self.assertIsNotNone(attempt.refund)
        self.assertLess(wallet.balance_msats, before)
        self.assertGreater(attempt.wallet_cost_msats, 0)
        assert attempt.refund is not None
        # The contract contains the invoice plus gateway allowance. Returning
        # the same contract cancels both values; only the two lnv2 federation
        # fees and the two generic mint finalizations remain.
        self.assertEqual(
            attempt.wallet_cost_msats,
            2 * lightning_fee_msats(attempt.contract_msats)
            + attempt.funding.mint_churn_cost_msats
            + attempt.refund.mint_churn_cost_msats,
        )

    def test_four_refunds_then_success_deliver_expected_empty_wallet_vector(self) -> None:
        wallet = MintV1WalletModel()
        wallet.claim(192_000)
        failed = [wallet.lightning_attempt(succeed=False) for _ in range(4)]
        success = wallet.lightning_attempt(succeed=True)
        self.assertTrue(all(attempt.wallet_cost_msats > 0 for attempt in failed))
        self.assertGreaterEqual(success.invoice_msats, 100_000)

    def test_sequential_golden_vectors_are_sufficient_and_minimal(self) -> None:
        vectors = {
            50: 86_016,
            100: 139_264,
            250: 296_960,
            500: 555_520,
            1_000: 1_071_104,
            2_000: 2_100_224,
            5_000: 5_183_488,
            10_000: 10_317_312,
            25_000: 25_712_640,
            50_000: 51_374_592,
            100_000: 102_692_352,
        }
        for target_sat, expected_gross_msats in vectors.items():
            gross = sequential_gross_price(target_sat * 1_000)
            self.assertEqual(gross, expected_gross_msats)
            self.assertGreaterEqual(
                sequential_settlement(gross).delivered_msats, target_sat * 1_000
            )
            self.assertLess(
                sequential_settlement(gross - 512).delivered_msats,
                target_sat * 1_000,
            )

    def test_whole_sat_economical_grid_causes_192_sat_jump(self) -> None:
        # lcm(1000 msat/sat, 512 msat/tier) = 64_000 msat. Requiring both
        # whole-sat display and economical-tier inputs therefore skips from
        # 128 sat (insufficient) to 192 sat (far above target).
        self.assertEqual(sequential_gross_price(100_000, grid_msats=64_000), 192_000)

    def test_five_claims_can_interleave_with_lightning_attempts(self) -> None:
        minimum, maximum, schedules = enumerate_completed_claim_interleavings(
            139_264, claims=5
        )
        self.assertGreater(schedules, 1)
        self.assertGreaterEqual(minimum.delivered_msats, 5 * 100_000)
        self.assertGreaterEqual(maximum.delivered_msats, minimum.delivered_msats)


def add_gross_price_args(parser: argparse.ArgumentParser) -> None:
    gross = parser.add_mutually_exclusive_group(required=True)
    gross.add_argument("--gross-sat", type=int)
    gross.add_argument("--gross-msat", type=int)


def gross_price_msats(args: argparse.Namespace) -> int:
    if args.gross_msat is not None:
        return checked_u64(args.gross_msat, "gross price")
    return checked_u64(args.gross_sat * 1_000, "gross price")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    repeat = subparsers.add_parser("repeat", help="apply equal claims repeatedly")
    add_gross_price_args(repeat)
    repeat.add_argument("--claims", type=int, required=True)

    mixed = subparsers.add_parser(
        "mixed", help="claim with a pre-existing single-tier wallet state"
    )
    add_gross_price_args(mixed)
    mixed.add_argument("--tier-msat", type=int, required=True)
    mixed.add_argument("--notes", type=int, required=True)

    settle = subparsers.add_parser(
        "settle", help="claim, apply failed send/refunds, then a successful send-all"
    )
    add_gross_price_args(settle)
    settle.add_argument("--failed-attempts", type=int, default=4)

    price = subparsers.add_parser(
        "price", help="derive the sequential empty-wallet baseline gross price"
    )
    price.add_argument("--net-sat", type=int, required=True)
    price.add_argument("--failed-attempts", type=int, default=4)
    price.add_argument("--grid-msat", type=int, default=512)

    interleave = subparsers.add_parser(
        "interleave",
        help="enumerate completed claims around non-overlapping lnv2 attempts",
    )
    add_gross_price_args(interleave)
    interleave.add_argument("--claims", type=int, required=True)
    interleave.add_argument("--failed-attempts", type=int, default=4)

    subparsers.add_parser("test", help="run the model's golden tests")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.command == "test":
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(ModelTests)
        return 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1

    if args.command == "price":
        target_msats = checked_u64(args.net_sat * 1_000, "net target")
        gross_msats = sequential_gross_price(
            target_msats, args.failed_attempts, args.grid_msat
        )
        delivered = sequential_settlement(
            gross_msats, args.failed_attempts
        ).delivered_msats
        print(f"gross: {format_msats(gross_msats)}")
        print(f"delivered: {format_msats(delivered)}")
        return 0

    if args.command == "interleave":
        gross_msats = gross_price_msats(args)
        minimum, maximum, schedules = enumerate_completed_claim_interleavings(
            gross_msats, args.claims, args.failed_attempts
        )
        print(f"schedules: {schedules}")
        print(f"minimum delivered: {format_msats(minimum.delivered_msats)}")
        print(f"minimum schedule: {' -> '.join(minimum.schedule)}")
        print(f"maximum delivered: {format_msats(maximum.delivered_msats)}")
        print(f"maximum schedule: {' -> '.join(maximum.schedule)}")
        return 0

    gross_msats = gross_price_msats(args)
    if args.command == "repeat":
        wallet = MintV1WalletModel()
        for index in range(1, args.claims + 1):
            print_result(index, wallet.claim(gross_msats))
        print(f"final wallet balance: {format_msats(wallet.balance_msats)}")
        return 0

    if args.command == "mixed":
        wallet = MintV1WalletModel({args.tier_msat: args.notes})
        print_result(1, wallet.claim(gross_msats))
        print(f"final wallet balance: {format_msats(wallet.balance_msats)}")
        return 0

    settlement = sequential_settlement(gross_msats, args.failed_attempts)
    print_result(1, settlement.claim)
    for index, attempt in enumerate(settlement.failed_attempts, 1):
        print_attempt(index, attempt)
    success = settlement.success
    print_attempt(args.failed_attempts + 1, success)
    print(f"delivered: {format_msats(success.invoice_msats)}")
    print(
        "remaining wallet balance: "
        f"{format_msats(success.funding.balance_after_msats)}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
