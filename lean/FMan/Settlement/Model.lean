/-!
# Settlement model

A transition system that mirrored Fleet Manager's retired refund-ledger
settlement machinery. The former implementation had acceptance and refund
writers, an in-memory acceptance mirror, and a startup rebuild. Current Fleet
Manager uses monotone admission and retains no refusal row.

This model backs the quote-settlement claim proof. Faithfulness to the Rust
is *not* established here; it is the separate obligation of
`crates/fman/specs/CLAIM-fleet-manager-quote-settlement-exclusive/proof.md`.

## Modelling decisions

* **The allocation lock is the atom.** The retired implementation ran every
  durable outcome writer under one allocation lock, so one hold is modelled as a
  list of effects computed from the pre-state and applied without interference.
  This encodes rather than proves the old writer enumeration.
* **Crashes truncate a hold.** A hold may be cut short at any effect boundary,
  after which volatile state is rebuilt from durable state. This is the
  "crash at any await" adversary.
* **Settlement effects are emitted eagerly.** The retired implementation handed
  work to background tasks that ran later and could be suppressed by an
  in-flight gate. Emitting at hand-off time over-approximated that implementation.
  No transfer to current Rust follows without renewed correspondence.
* **Admission is adversarial.** The former capacity and admission gates are left
  as a free choice rather than modelled from a port grid.
-/

namespace FMan.Settlement

abbrev QuoteId := Nat
abbrev SeatId := Nat
abbrev FiId := Nat
abbrev FedId := Nat
/-- The serialized refund transaction, as bytes. -/
abbrev Txn := Nat

/-- A modeled row of the former `seats` table. Its quote key and creation fields
are immutable; decommissioning is modelled separately. -/
structure SeatRow where
  quote : QuoteId
  seat : SeatId
  fi : FiId
  deriving DecidableEq, Repr

/-- A modeled row of the retired `refund_ledger` table, keyed by quote. -/
structure RefundRow where
  quote : QuoteId
  fed : FedId
  txn : Txn
  deriving DecidableEq, Repr

/-- Committed SQLite state. -/
structure Durable where
  seats : List SeatRow
  refunds : List RefundRow
  /-- Modeled decommissioned seats. -/
  decommissioned : List SeatId
  deriving Repr

def Durable.seatFor (d : Durable) (q : QuoteId) : Option SeatRow :=
  d.seats.find? (fun r => r.quote == q)

def Durable.refundFor (d : Durable) (q : QuoteId) : Option RefundRow :=
  d.refunds.find? (fun r => r.quote == q)

/-- The retired implementation's in-memory acceptance mirror. Other former
in-memory indexes and liveness gates added no modeled outcome. -/
structure Volatile where
  accepted : List SeatRow
  deriving Repr

def Volatile.acceptedFor (v : Volatile) (q : QuoteId) : Option SeatRow :=
  v.accepted.find? (fun r => r.quote == q)

/-- Everything that leaves the process. `refusalResp` carries the signed
`CreateSeatResponse` commitment, whose refund transaction is `some t` for a paid
refusal and `none` for a free one. -/
inductive Event where
  | acceptResp (q : QuoteId) (s : SeatId)
  | refusalResp (q : QuoteId) (t : Option Txn)
  | submitRefund (q : QuoteId) (t : Txn)
  | claim (q : QuoteId)
  deriving DecidableEq, Repr

structure State where
  durable : Durable
  vol : Volatile
  /-- Emitted events, newest first. -/
  log : List Event
  deriving Repr

/-- One primitive effect of a lock hold. -/
inductive Effect where
  | dbCreateSeat (q : QuoteId) (s : SeatId) (fi : FiId)
  | dbRecordRefund (q : QuoteId) (fed : FedId) (t : Txn)
  | volInsertAccepted (q : QuoteId) (s : SeatId) (fi : FiId)
  | emit (e : Event)
  deriving DecidableEq, Repr

def Effect.apply (st : State) : Effect → State
  | .dbCreateSeat q s fi =>
      { st with durable := { st.durable with seats := ⟨q, s, fi⟩ :: st.durable.seats } }
  | .dbRecordRefund q fed t =>
      { st with durable := { st.durable with refunds := ⟨q, fed, t⟩ :: st.durable.refunds } }
  | .volInsertAccepted q s fi =>
      { st with vol := { st.vol with accepted := ⟨q, s, fi⟩ :: st.vol.accepted } }
  | .emit e => { st with log := e :: st.log }

def runEffects (st : State) (es : List Effect) : State :=
  es.foldl Effect.apply st

/-- A modeled request admitted by the retired service boundary. `payment` is
`none` for a free quote and `some (fed, txn)` for a paid one. The transaction's
binding to verified payment is a model input premise, not a theorem about current
Rust verification. -/
structure CreateInput where
  quote : QuoteId
  fi : FiId
  /-- The modeled accepted seat identifier. -/
  seat : SeatId
  payment : Option (FedId × Txn)
  deriving Repr

/-- The modeled admission decision, left adversarial. -/
inductive Admission where
  | admit
  | refuse
  deriving DecidableEq, Repr

/-- The retired lock hold's modeled effect order: accepted replay, refund-ledger
replay, admission, and then refusal or seat creation. -/
def plan (st : State) (inp : CreateInput) (adm : Admission) : List Effect :=
  match st.vol.acceptedFor inp.quote with
  | some row => [.emit (.claim inp.quote), .emit (.acceptResp inp.quote row.seat)]
  | none =>
    match st.durable.refundFor inp.quote with
    | some r =>
        [.emit (.submitRefund inp.quote r.txn), .emit (.refusalResp inp.quote (some r.txn))]
    | none =>
      match adm with
      | .refuse =>
        match inp.payment with
        | some (fed, t) =>
            [.dbRecordRefund inp.quote fed t,
             .emit (.submitRefund inp.quote t),
             .emit (.refusalResp inp.quote (some t))]
        | none => [.emit (.refusalResp inp.quote none)]
      | .admit =>
          [.dbCreateSeat inp.quote inp.seat inp.fi,
           .volInsertAccepted inp.quote inp.seat inp.fi,
           .emit (.claim inp.quote),
           .emit (.acceptResp inp.quote inp.seat)]

/-- The retired startup rebuild of the acceptance mirror from durable seats. -/
def rebuild (d : Durable) : Volatile := { accepted := d.seats }

def State.reboot (st : State) : State :=
  { st with vol := rebuild st.durable }

/-- The retired decommission behavior: retain the seat and acceptance mirror. -/
def State.decommission (st : State) (s : SeatId) : State :=
  { st with durable := { st.durable with decommissioned := s :: st.durable.decommissioned } }

inductive Step : State → State → Prop where
  /-- A lock hold that ran to completion. -/
  | create (st : State) (inp : CreateInput) (adm : Admission) :
      Step st (runEffects st (plan st inp adm))
  /-- A lock hold cut short by a crash after `n` effects, followed by restart. -/
  | createCrash (st : State) (inp : CreateInput) (adm : Admission) (n : Nat) :
      Step st (runEffects st ((plan st inp adm).take n)).reboot
  /-- A crash outside any hold. -/
  | crash (st : State) : Step st st.reboot
  | decommission (st : State) (s : SeatId) : Step st (st.decommission s)

def Initial : State := { durable := ⟨[], [], []⟩, vol := ⟨[]⟩, log := [] }

inductive Reachable : State → Prop where
  | init : Reachable Initial
  | step {s s' : State} : Reachable s → Step s s' → Reachable s'

end FMan.Settlement
