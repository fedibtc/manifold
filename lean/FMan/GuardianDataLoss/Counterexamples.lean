import FMan.GuardianDataLoss.Claims

/-!
# The model can fail

Each namespace mutates one load-bearing ordering and gives an executable
reachable trace whose newest-first log is exactly `[wipe, invite]`. These are
model tests: changing event order or the relevant guard makes `rfl`/`decide`
stop proving the witness.
-/

namespace FMan.GuardianDataLoss.Counterexamples

open FMan.GuardianDataLoss

def spawned : State := runEffects Initial (supervisorSpawnPlan Initial)
def coded : State := runEffects spawned dkgCodePlan
def observed : State := runEffects coded invitePlan

/-! ## Mutation 1 — restart consults the fresh attempt

A literal move of the lifetime `consensusObserved` Boolean would still refuse.
The unsafe mutation represented here is the meaningful late-check bug: stop and
append first, then consult the *new current attempt's* observation field. The
fresh attempt is necessarily unobserved, so the prior lifetime observation is
lost and W2 runs. -/

namespace LateRestartCheck

/-- Stop, append, then (incorrectly) accept the fresh row's false observation
instead of the pre-append lifetime mirror, and wipe. -/
def restartLate (st : State) : State :=
  runEffects st [.setChildRunning false, .appendCodeless, .emit .wipe]

inductive Step : State → State → Prop where
  | spawn : Step Initial spawned
  | code : Step spawned coded
  | invite : Step coded observed
  | restart : Step observed (restartLate observed)

inductive Reachable : State → Prop where
  | init : Reachable Initial
  | step {s s'} : Reachable s → Step s s' → Reachable s'

def bad : State := restartLate observed

theorem bad_reachable : Reachable bad :=
  .step (.step (.step (.step .init .spawn) .code) .invite) .restart

theorem wipe_follows_invite : bad.log = [.wipe, .invite] := rfl
theorem bad_shape : ∃ pre middle post,
    bad.log = pre ++ Event.wipe :: middle ++ Event.invite :: post := by
  exact ⟨[], [], [], rfl⟩

end LateRestartCheck

/-! ## Mutation 2 — W1 is not consumed

After a crash leaves a codeless row, the first spawn wipes it. If the supervisor
keeps `wipeFirst = true`, that child can acquire a code and emit an invite, and
its next crash-respawn performs W1 again. -/

namespace WipeFirstNotConsumed

def firstSpawn (st : State) : State :=
  runEffects st [.emit .wipe, .setChildRunning true]

def respawn (st : State) : State :=
  runEffects st [.emit .wipe, .setChildRunning true]

def pending : State := (runEffects spawned [.appendCodeless]).reboot
def first : State := firstSpawn pending
def recoded : State := Effect.apply first .recordGuardianCode
def invited : State := runEffects recoded invitePlan
def bad : State := respawn invited

inductive Step : State → State → Prop where
  | spawnInitial : Step Initial spawned
  | appendCrash : Step spawned pending
  | firstSpawn : Step pending first
  | code : Step first recoded
  | invite : Step recoded invited
  | respawn : Step invited bad

inductive Reachable : State → Prop where
  | init : Reachable Initial
  | step {s s'} : Reachable s → Step s s' → Reachable s'

theorem bad_reachable : Reachable bad :=
  .step (.step (.step (.step (.step (.step .init .spawnInitial) .appendCrash)
    .firstSpawn) .code) .invite) .respawn

theorem wipe_follows_invite : bad.log = [.wipe, .invite, .wipe] := rfl
theorem bad_shape : ∃ pre middle post,
    bad.log = pre ++ Event.wipe :: middle ++ Event.invite :: post := by
  exact ⟨[], [], [.wipe], rfl⟩

end WipeFirstNotConsumed

/-! ## Mutation 3 — invite leaves before observation commits

The bad invite plan emits first. A daemon crash at the following await boundary
rebuilds a false mirror from the still-false durable bit, so an unreachable
restart appends and performs W2. -/

namespace InviteBeforeObservation

def inviteThenCrash (st : State) : State :=
  (runEffects st [.emit .invite]).reboot

def invited : State := inviteThenCrash coded
def bad : State := runEffects invited (restartPlan invited .unreachable)

inductive Step : State → State → Prop where
  | spawn : Step Initial spawned
  | code : Step spawned coded
  | inviteCrash : Step coded invited
  | restart : Step invited bad

inductive Reachable : State → Prop where
  | init : Reachable Initial
  | step {s s'} : Reachable s → Step s s' → Reachable s'

theorem bad_reachable : Reachable bad :=
  .step (.step (.step (.step .init .spawn) .code) .inviteCrash) .restart

theorem wipe_follows_invite : bad.log = [.wipe, .invite] := rfl
theorem bad_shape : ∃ pre middle post,
    bad.log = pre ++ Event.wipe :: middle ++ Event.invite :: post := by
  exact ⟨[], [], [], rfl⟩

end InviteBeforeObservation

/-- The real model rejects the event shape witnessed above. -/
theorem no_bad_shape_in_real_model
    (allowed : State → Probe → Prop)
    (a2 : ∀ st p, allowed st p → FedimintdA2 st p)
    {st : State} (h : Reachable allowed st) :
    ¬ ∃ pre middle post,
      st.log = pre ++ Event.wipe :: middle ++ Event.invite :: post :=
  no_invite_then_wipe allowed a2 h

end FMan.GuardianDataLoss.Counterexamples
