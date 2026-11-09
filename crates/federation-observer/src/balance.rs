//! Module balances from the full session history: the ecash the mint has
//! issued and not yet redeemed, the bitcoin the wallet has taken in and paid
//! out, and the funds locked in open Lightning contracts.
//!
//! Every figure is a sum over accepted transactions, so it is only right
//! when the archive holds every session from 0 on.

use std::collections::BTreeMap;

use anyhow::{Context as _, bail};
use fedimint_core::core::ModuleInstanceId;
use fedimint_core::epoch::ConsensusItem;
use fedimint_core::session_outcome::SessionOutcome;
use fedimint_ln_common::contracts::{ContractId, IdentifiableContract as _};
use fedimint_ln_common::{LightningInput, LightningOutput, LightningOutputV0};
use fedimint_mint_common::{MintInput, MintOutput};
use fedimint_wallet_common::{WalletInput, WalletOutput, WalletOutputV0};

/// Instance ids of the modules whose amounts this report decodes.
#[derive(Debug, Clone, Copy)]
pub struct Instances {
    pub mint: ModuleInstanceId,
    pub wallet: Option<ModuleInstanceId>,
    pub ln: Option<ModuleInstanceId>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Flow {
    pub count: u64,
    pub msat: u64,
}

impl Flow {
    fn add(&mut self, msat: u64) {
        self.count += 1;
        self.msat += msat;
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Balances {
    /// Notes issued: mint outputs.
    pub mint_issued: Flow,
    /// Notes redeemed: mint inputs.
    pub mint_redeemed: Flow,
    /// Deposits claimed: wallet inputs, by the value of the deposit output.
    pub peg_in: Flow,
    /// Withdrawals: peg-out amounts, without their on-chain fees.
    pub peg_out: Flow,
    /// On-chain fees of peg-outs and their fee bumps.
    pub peg_out_fees: Flow,
    /// LNv1 contract balances by id, in msat. Outputs fund a contract, and
    /// inputs spend from it, not necessarily all at once.
    pub ln_contracts: BTreeMap<ContractId, u64>,
    /// Inputs minus outputs of the transactions that touch only the modules
    /// above: the fees they paid.
    pub fees: Flow,
    /// Inputs minus outputs, counting only the modules above, of the
    /// transactions that also touch another module. Their sum is what moved
    /// into the other modules, net of what came back, plus these
    /// transactions' fees.
    pub to_other_modules: SignedFlow,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SignedFlow {
    pub count: u64,
    pub msat: i128,
}

impl Balances {
    pub fn ecash_outstanding_msat(&self) -> i128 {
        i128::from(self.mint_issued.msat) - i128::from(self.mint_redeemed.msat)
    }

    /// Bitcoin the wallet should still hold.
    pub fn wallet_msat(&self) -> i128 {
        i128::from(self.peg_in.msat)
            - i128::from(self.peg_out.msat)
            - i128::from(self.peg_out_fees.msat)
    }

    /// LNv1 contracts with a balance left.
    pub fn ln_open(&self) -> Flow {
        let mut open = Flow::default();
        for msat in self.ln_contracts.values().filter(|msat| **msat > 0) {
            open.add(*msat);
        }
        open
    }

    pub fn add_session(
        &mut self,
        session: u64,
        outcome: &SessionOutcome,
        instances: Instances,
    ) -> anyhow::Result<()> {
        for accepted in &outcome.items {
            let ConsensusItem::Transaction(tx) = &accepted.item else {
                continue;
            };
            let txid = tx.tx_hash();
            let mut net: i128 = 0;
            let mut other = false;
            for input in &tx.inputs {
                let id = input.module_instance_id();
                if id == instances.mint {
                    match input.as_any().downcast_ref::<MintInput>() {
                        Some(MintInput::V0(note)) => {
                            self.mint_redeemed.add(note.amount.msats);
                            net += i128::from(note.amount.msats);
                        }
                        other => bail!("session {session} tx {txid}: mint input {other:?}"),
                    }
                } else if Some(id) == instances.wallet {
                    let value = match input.as_any().downcast_ref::<WalletInput>() {
                        Some(WalletInput::V0(proof)) => proof.tx_output().value,
                        Some(WalletInput::V1(input)) => input.tx_out.value,
                        other => bail!("session {session} tx {txid}: wallet input {other:?}"),
                    };
                    self.peg_in.add(value.to_sat() * 1000);
                    net += i128::from(value.to_sat() * 1000);
                } else if Some(id) == instances.ln {
                    let Some(LightningInput::V0(input)) =
                        input.as_any().downcast_ref::<LightningInput>()
                    else {
                        bail!("session {session} tx {txid}: undecodable ln input");
                    };
                    let balance =
                        self.ln_contracts
                            .get_mut(&input.contract_id)
                            .with_context(|| {
                                format!(
                                    "session {session} tx {txid}: spends unknown ln contract {}",
                                    input.contract_id
                                )
                            })?;
                    *balance = balance.checked_sub(input.amount.msats).with_context(|| {
                        format!(
                            "session {session} tx {txid}: spends {} msat from ln contract {} holding {balance} msat",
                            input.amount.msats, input.contract_id
                        )
                    })?;
                    net += i128::from(input.amount.msats);
                } else {
                    other = true;
                }
            }
            for output in &tx.outputs {
                let id = output.module_instance_id();
                if id == instances.mint {
                    match output.as_any().downcast_ref::<MintOutput>() {
                        Some(MintOutput::V0(note)) => {
                            self.mint_issued.add(note.amount.msats);
                            net -= i128::from(note.amount.msats);
                        }
                        other => bail!("session {session} tx {txid}: mint output {other:?}"),
                    }
                } else if Some(id) == instances.wallet {
                    match output.as_any().downcast_ref::<WalletOutput>() {
                        Some(WalletOutput::V0(WalletOutputV0::PegOut(peg_out))) => {
                            self.peg_out.add(peg_out.amount.to_sat() * 1000);
                            self.peg_out_fees.add(peg_out.fees.amount().to_sat() * 1000);
                            net -= i128::from(
                                (peg_out.amount + peg_out.fees.amount()).to_sat() * 1000,
                            );
                        }
                        Some(WalletOutput::V0(WalletOutputV0::Rbf(rbf))) => {
                            self.peg_out_fees.add(rbf.fees.amount().to_sat() * 1000);
                            net -= i128::from(rbf.fees.amount().to_sat() * 1000);
                        }
                        other => bail!("session {session} tx {txid}: wallet output {other:?}"),
                    }
                } else if Some(id) == instances.ln {
                    match output.as_any().downcast_ref::<LightningOutput>() {
                        Some(LightningOutput::V0(LightningOutputV0::Contract(c))) => {
                            // Funding an existing contract again adds to it.
                            *self
                                .ln_contracts
                                .entry(c.contract.contract_id())
                                .or_default() += c.amount.msats;
                            net -= i128::from(c.amount.msats);
                        }
                        Some(LightningOutput::V0(
                            LightningOutputV0::Offer(_) | LightningOutputV0::CancelOutgoing { .. },
                        )) => {}
                        other => bail!("session {session} tx {txid}: ln output {other:?}"),
                    }
                } else {
                    other = true;
                }
            }
            if other {
                self.to_other_modules.count += 1;
                self.to_other_modules.msat += net;
            } else {
                let fee = u64::try_from(net).with_context(|| {
                    format!(
                        "session {session} tx {txid}: outputs exceed inputs by {} msat",
                        -net
                    )
                })?;
                self.fees.add(fee);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use fedimint_core::bitcoin::hashes::{Hash as _, sha256};
    use fedimint_core::core::{DynInput, DynOutput};
    use fedimint_core::secp256k1::{PublicKey, Secp256k1, SecretKey};
    use fedimint_core::session_outcome::AcceptedItem;
    use fedimint_core::transaction::{Transaction, TransactionSignature};
    use fedimint_core::{Amount, PeerId, bitcoin};
    use fedimint_ln_common::contracts::Contract;
    use fedimint_ln_common::contracts::outgoing::OutgoingContract;
    use fedimint_ln_common::{ContractOutput, LightningInputV0};
    use fedimint_mint_common::{BlindNonce, MintInputV0, MintOutputV0, Nonce, Note};
    use fedimint_wallet_common::{PegOut, PegOutFees, Rbf, WalletInputV1};

    use super::*;

    const MINT: ModuleInstanceId = 1;
    const WALLET: ModuleInstanceId = 2;
    const LN: ModuleInstanceId = 0;
    const INSTANCES: Instances = Instances {
        mint: MINT,
        wallet: Some(WALLET),
        ln: Some(LN),
    };

    fn key(n: u8) -> PublicKey {
        SecretKey::from_slice(&[n; 32])
            .expect("valid key")
            .public_key(&Secp256k1::new())
    }

    fn point(n: u8) -> tbs::BlindedMessage {
        tbs::blind_message(tbs::Message::from_bytes(&[n]), tbs::BlindingKey::random())
    }

    fn note_in(msat: u64) -> DynInput {
        let note = Note {
            nonce: Nonce(key(1)),
            signature: tbs::Signature(point(1).0),
        };
        DynInput::from_typed(
            MINT,
            MintInput::V0(MintInputV0 {
                amount: Amount::from_msats(msat),
                note,
            }),
        )
    }

    fn note_out(msat: u64) -> DynOutput {
        DynOutput::from_typed(
            MINT,
            MintOutput::V0(MintOutputV0 {
                amount: Amount::from_msats(msat),
                blind_nonce: BlindNonce(point(2)),
            }),
        )
    }

    fn contract() -> Contract {
        Contract::Outgoing(OutgoingContract {
            hash: sha256::Hash::hash(b"invoice"),
            gateway_key: key(2),
            timelock: 100,
            user_key: key(3),
            cancelled: false,
        })
    }

    fn fund(msat: u64) -> DynOutput {
        DynOutput::from_typed(
            LN,
            LightningOutput::V0(LightningOutputV0::Contract(ContractOutput {
                amount: Amount::from_msats(msat),
                contract: contract(),
            })),
        )
    }

    fn spend(msat: u64) -> DynInput {
        DynInput::from_typed(
            LN,
            LightningInput::V0(LightningInputV0 {
                contract_id: contract().contract_id(),
                amount: Amount::from_msats(msat),
                witness: None,
            }),
        )
    }

    fn peg_in(sat: u64) -> DynInput {
        DynInput::from_typed(
            WALLET,
            WalletInput::V1(WalletInputV1 {
                outpoint: bitcoin::OutPoint::null(),
                tweak_key: key(4),
                tx_out: bitcoin::TxOut {
                    value: bitcoin::Amount::from_sat(sat),
                    script_pubkey: bitcoin::ScriptBuf::new(),
                },
            }),
        )
    }

    fn fees(sat: u64) -> PegOutFees {
        // Weight 4 is one vbyte, so the fee is `sats_per_kvb / 1000`.
        PegOutFees::new(sat * 1000, 4)
    }

    fn peg_out(sat: u64, fee_sat: u64) -> DynOutput {
        let recipient = "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq"
            .parse()
            .expect("valid address");
        DynOutput::from_typed(
            WALLET,
            WalletOutput::V0(WalletOutputV0::PegOut(PegOut {
                recipient,
                amount: bitcoin::Amount::from_sat(sat),
                fees: fees(fee_sat),
            })),
        )
    }

    fn bump(fee_sat: u64) -> DynOutput {
        DynOutput::from_typed(
            WALLET,
            WalletOutput::V0(WalletOutputV0::Rbf(Rbf {
                fees: fees(fee_sat),
                txid: bitcoin::Txid::all_zeros(),
            })),
        )
    }

    fn session(txs: Vec<(Vec<DynInput>, Vec<DynOutput>)>) -> SessionOutcome {
        SessionOutcome {
            items: txs
                .into_iter()
                .map(|(inputs, outputs)| AcceptedItem {
                    item: ConsensusItem::Transaction(Transaction {
                        inputs,
                        outputs,
                        nonce: [0; 8],
                        signatures: TransactionSignature::NaiveMultisig(vec![]),
                    }),
                    peer: PeerId::from(0),
                })
                .collect(),
        }
    }

    #[test]
    fn sums_flows_across_modules() {
        let mut b = Balances::default();
        // Peg in 10_000 sat and mint 9_990_000 msat of notes.
        b.add_session(
            0,
            &session(vec![(
                vec![peg_in(10_000)],
                vec![note_out(8_000_000), note_out(1_990_000)],
            )]),
            INSTANCES,
        )
        .unwrap();
        // Spend a note into an outgoing contract, which the gateway claims
        // in two partial spends, the second one later; then peg out.
        b.add_session(
            1,
            &session(vec![
                (
                    vec![note_in(8_000_000)],
                    vec![fund(7_000_000), note_out(900_000)],
                ),
                (vec![spend(3_000_000)], vec![note_out(2_999_000)]),
                (vec![note_in(1_990_000)], vec![peg_out(1_500, 30), bump(7)]),
            ]),
            INSTANCES,
        )
        .unwrap();

        assert_eq!(
            b.mint_issued,
            Flow {
                count: 4,
                msat: 13_889_000
            }
        );
        assert_eq!(
            b.mint_redeemed,
            Flow {
                count: 2,
                msat: 9_990_000
            }
        );
        assert_eq!(b.ecash_outstanding_msat(), 3_899_000);
        assert_eq!(
            b.peg_in,
            Flow {
                count: 1,
                msat: 10_000_000
            }
        );
        assert_eq!(
            b.peg_out,
            Flow {
                count: 1,
                msat: 1_500_000
            }
        );
        assert_eq!(
            b.peg_out_fees,
            Flow {
                count: 2,
                msat: 37_000
            }
        );
        assert_eq!(b.wallet_msat(), 8_463_000);
        // A partial spend leaves the rest of the contract open.
        assert_eq!(
            b.ln_open(),
            Flow {
                count: 1,
                msat: 4_000_000
            }
        );
        assert_eq!(
            b.fees,
            Flow {
                count: 4,
                msat: 10_000 + 100_000 + 1_000 + 453_000
            }
        );

        // A deposit into a module this report does not decode, for example
        // the stability pool, is not a fee.
        let deposit = DynOutput::from_typed(
            9,
            MintOutput::V0(MintOutputV0 {
                amount: Amount::from_msats(1),
                blind_nonce: BlindNonce(point(3)),
            }),
        );
        b.add_session(
            2,
            &session(vec![
                (vec![spend(4_000_000)], vec![]),
                (vec![note_in(50_000)], vec![deposit, note_out(20_000)]),
            ]),
            INSTANCES,
        )
        .unwrap();
        assert_eq!(b.ln_open(), Flow::default());
        assert_eq!(
            b.fees,
            Flow {
                count: 5,
                msat: 4_564_000
            }
        );
        assert_eq!(
            b.to_other_modules,
            SignedFlow {
                count: 1,
                msat: 30_000
            }
        );
        assert_eq!(b.ecash_outstanding_msat(), 3_899_000 - 30_000);
    }

    #[test]
    fn rejects_overspent_and_unknown_contracts() {
        let mut b = Balances::default();
        assert!(
            b.add_session(0, &session(vec![(vec![spend(1)], vec![])]), INSTANCES)
                .is_err()
        );
        b.add_session(
            1,
            &session(vec![(vec![note_in(5_000)], vec![fund(5_000)])]),
            INSTANCES,
        )
        .unwrap();
        assert!(
            b.add_session(2, &session(vec![(vec![spend(5_001)], vec![])]), INSTANCES)
                .is_err()
        );
    }
}
