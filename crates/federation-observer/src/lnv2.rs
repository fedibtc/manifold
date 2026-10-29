//! LNv2 contract history from session outcomes.

use std::collections::BTreeMap;
use std::fmt;

use fedimint_core::core::ModuleInstanceId;
use fedimint_core::epoch::ConsensusItem;
use fedimint_core::session_outcome::SessionOutcome;
use fedimint_core::{OutPoint, TransactionId};
use fedimint_lnv2_common::{
    LightningConsensusItem, LightningInput, LightningInputV0, LightningOutput, LightningOutputV0,
    OutgoingWitness,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// User pays out through a gateway.
    Send,
    /// Gateway pays a user in.
    Receive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settlement {
    /// Send: the gateway claimed with the preimage, so it paid the invoice.
    Paid,
    /// Send: the contract expired and the user took the funds back.
    Refunded,
    /// Send: the gateway signed a forfeit because it could not pay.
    Cancelled,
    /// Receive: the incoming contract was spent, normally by the receiver.
    /// The gateway gets it back instead when the preimage does not decrypt;
    /// the input alone does not tell the two apart.
    Spent,
}

impl fmt::Display for Settlement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Paid => "paid",
            Self::Refunded => "refunded",
            Self::Cancelled => "cancelled",
            Self::Spent => "spent",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    Funded {
        direction: Direction,
        amount_msat: u64,
        /// The gateway's key: the claim key of an outgoing contract, the
        /// refund key of an incoming one.
        gateway: String,
    },
    Settled(Settlement),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub txid: TransactionId,
    /// The contract this event funds or spends.
    pub contract: OutPoint,
    pub kind: EventKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEvents {
    pub session: u64,
    /// The highest LNv2 unix time vote in the session, in seconds.
    pub time: Option<u64>,
    pub events: Vec<Event>,
}

pub fn session_events(
    session: u64,
    outcome: &SessionOutcome,
    lnv2: ModuleInstanceId,
) -> SessionEvents {
    let mut time = None;
    let mut events = vec![];
    for accepted in &outcome.items {
        match &accepted.item {
            ConsensusItem::Module(item) if item.module_instance_id() == lnv2 => {
                if let Some(LightningConsensusItem::UnixTimeVote(t)) =
                    item.as_any().downcast_ref::<LightningConsensusItem>()
                {
                    time = Some(time.map_or(*t, |prev: u64| prev.max(*t)));
                }
            }
            ConsensusItem::Transaction(tx) => {
                let txid = tx.tx_hash();
                for input in tx.inputs.iter().filter(|i| i.module_instance_id() == lnv2) {
                    let Some(LightningInput::V0(input)) =
                        input.as_any().downcast_ref::<LightningInput>()
                    else {
                        continue;
                    };
                    let (contract, settlement) = match input {
                        LightningInputV0::Outgoing(op, OutgoingWitness::Claim(_)) => {
                            (*op, Settlement::Paid)
                        }
                        LightningInputV0::Outgoing(op, OutgoingWitness::Refund) => {
                            (*op, Settlement::Refunded)
                        }
                        LightningInputV0::Outgoing(op, OutgoingWitness::Cancel(_)) => {
                            (*op, Settlement::Cancelled)
                        }
                        LightningInputV0::Incoming(op, _) => (*op, Settlement::Spent),
                    };
                    events.push(Event {
                        txid,
                        contract,
                        kind: EventKind::Settled(settlement),
                    });
                }
                for (out_idx, output) in tx.outputs.iter().enumerate() {
                    if output.module_instance_id() != lnv2 {
                        continue;
                    }
                    let Some(LightningOutput::V0(output)) =
                        output.as_any().downcast_ref::<LightningOutput>()
                    else {
                        continue;
                    };
                    let (direction, amount_msat, gateway) = match output {
                        LightningOutputV0::Outgoing(c) => {
                            (Direction::Send, c.amount.msats, c.claim_pk.to_string())
                        }
                        LightningOutputV0::Incoming(c) => (
                            Direction::Receive,
                            c.commitment.amount.msats,
                            // The receiver claims an incoming contract, and the
                            // gateway holds the refund key.
                            c.commitment.refund_pk.to_string(),
                        ),
                    };
                    events.push(Event {
                        txid,
                        contract: OutPoint {
                            txid,
                            out_idx: out_idx as u64,
                        },
                        kind: EventKind::Funded {
                            direction,
                            amount_msat,
                            gateway,
                        },
                    });
                }
            }
            _ => {}
        }
    }
    SessionEvents {
        session,
        time,
        events,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contract {
    pub contract: OutPoint,
    pub direction: Direction,
    pub amount_msat: u64,
    pub gateway: String,
    pub funded_session: u64,
    pub funded_time: Option<u64>,
    /// `None` while the contract is open, or when its settlement lies
    /// outside the archived sessions.
    pub settled: Option<(u64, Settlement)>,
}

/// Join funding and settlement events by contract, in funding order.
/// Settlements of contracts funded before the first archived session are
/// dropped, because their amount and gateway are unknown.
pub fn contracts(sessions: impl IntoIterator<Item = SessionEvents>) -> Vec<Contract> {
    let mut out: Vec<Contract> = vec![];
    let mut by_outpoint: BTreeMap<OutPoint, usize> = BTreeMap::new();
    let mut settlements: BTreeMap<OutPoint, (u64, Settlement)> = BTreeMap::new();
    for session in sessions {
        for event in session.events {
            match event.kind {
                EventKind::Funded {
                    direction,
                    amount_msat,
                    gateway,
                } => {
                    by_outpoint.insert(event.contract, out.len());
                    out.push(Contract {
                        contract: event.contract,
                        direction,
                        amount_msat,
                        gateway,
                        funded_session: session.session,
                        funded_time: session.time,
                        settled: None,
                    });
                }
                EventKind::Settled(s) => {
                    settlements.insert(event.contract, (session.session, s));
                }
            }
        }
    }
    for (outpoint, settled) in settlements {
        if let Some(&i) = by_outpoint.get(&outpoint) {
            out[i].settled = Some(settled);
        }
    }
    out
}

/// Format unix seconds as `YYYY-MM-DD HH:MM` UTC.
pub fn format_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil-from-days, Howard Hinnant's algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60
    )
}

#[cfg(test)]
mod tests {
    use fedimint_core::encoding::Decodable as _;

    use super::*;

    fn op(byte: u8, out_idx: u64) -> OutPoint {
        let txid = TransactionId::consensus_decode_whole(&[byte; 32], &Default::default()).unwrap();
        OutPoint { txid, out_idx }
    }

    fn funded(contract: OutPoint, direction: Direction, amount_msat: u64) -> Event {
        Event {
            txid: contract.txid,
            contract,
            kind: EventKind::Funded {
                direction,
                amount_msat,
                gateway: "gw".into(),
            },
        }
    }

    fn settled(contract: OutPoint, s: Settlement) -> Event {
        Event {
            txid: op(0xee, 0).txid,
            contract,
            kind: EventKind::Settled(s),
        }
    }

    #[test]
    fn joins_settlement_to_the_funded_outpoint_across_sessions() {
        // Two contracts in one transaction differ only in out_idx; a join by
        // txid alone would give both the same result.
        let a = op(1, 0);
        let b = op(1, 1);
        let orphan = op(9, 0);
        let rows = contracts([
            SessionEvents {
                session: 10,
                time: Some(100),
                events: vec![
                    funded(a, Direction::Send, 5_000),
                    funded(b, Direction::Receive, 7_000),
                ],
            },
            SessionEvents {
                session: 11,
                time: Some(800),
                events: vec![settled(orphan, Settlement::Paid)],
            },
            SessionEvents {
                session: 12,
                time: None,
                events: vec![settled(b, Settlement::Spent)],
            },
            SessionEvents {
                session: 13,
                time: None,
                events: vec![settled(a, Settlement::Cancelled)],
            },
        ]);
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (rows[0].contract, rows[0].settled),
            (a, Some((13, Settlement::Cancelled)))
        );
        assert_eq!(rows[0].funded_time, Some(100));
        assert_eq!(
            (rows[1].contract, rows[1].settled),
            (b, Some((12, Settlement::Spent)))
        );
        assert_eq!(rows[1].amount_msat, 7_000);
    }

    #[test]
    fn open_contract_has_no_settlement() {
        let rows = contracts([SessionEvents {
            session: 3,
            time: None,
            events: vec![funded(op(2, 0), Direction::Send, 1)],
        }]);
        assert_eq!(rows[0].settled, None);
    }

    #[test]
    fn formats_utc() {
        // `date -u -d @0`, `@951782400` (leap day 2000), `@1793254440`.
        assert_eq!(format_utc(0), "1970-01-01 00:00");
        assert_eq!(format_utc(951_782_400), "2000-02-29 00:00");
        assert_eq!(format_utc(1_793_254_440), "2026-10-29 06:14");
    }
}
