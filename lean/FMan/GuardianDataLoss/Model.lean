/-!
# Guardian data-loss model

A transition system mirroring the per-seat safety machinery in `Seat::start`,
`SeatLoop`, and `supervise`. It backs the
`CLAIM-fleet-manager-preserves-published-guardian-data` record.
Faithfulness to the Rust is not proved here; it remains a correspondence
obligation outside this model.

## Modelling decisions

* **One seat is modelled.** The claim is pointwise in a typed seat id.  Path
  construction and database uniqueness (record lemma L5) are therefore outside
  the model; every `wipe` event denotes deletion of this seat's data directory.
* **Attempts are newest first.** Consing a codeless row represents SQL's append
  of a fresh, greater `attempt_no`; the tail is never changed.  The current
  row's set-once guardian-code field is abstracted to a `Bool`.
* **Actor commands are effect plans.** One `SeatLoop` consumes a command to
  completion. A crash can truncate a plan at any effect boundary, but another
  command cannot interleave. This encodes the single-consumer ownership that
  keeps a probe out of `restart_dkg`'s decision-to-wipe interval.
* **Probe results are adversarial.** A consensus result names whether the API
  belongs to the current or reaped prior attempt. `FedimintdA2` is an explicit
  hypothesis on every such result, rather than an axiom command or a silent
  restriction of the datatype.
* **Only two effects emit `wipe`.** `restartPlan` is W2. `supervisorSpawnPlan`
  is W1 and clears `wipeFirst` before a child can serve. Filesystem failures are
  represented by crash-truncation; retrying a failed wipe only adds earlier
  wipes, while a successful first spawn consumes the flag.
* **Unrelated verbs are stuttering steps.** Decommission, shutdown, malformed
  RPCs, caller cancellation, setup inputs, and probe errors cannot emit either
  event. Successful invite and status exits share the same record-before-emit
  plan; this deliberately forgets which wire method carried the invite.
-/

namespace FMan.GuardianDataLoss

/-- The only durable attempt fact used by the argument. -/
structure Attempt where
  guardianCode : Bool
  deriving DecidableEq, Repr

/-- Committed SQLite facts. `attempts` is newest first. -/
structure Durable where
  attempts : List Attempt
  /-- The existential, lifetime observation loaded by
  `Db::consensus_ever_observed`. -/
  consensusObserved : Bool
  deriving DecidableEq, Repr

def Durable.latest (d : Durable) : Option Attempt := d.attempts.head?

def Durable.latestHasCode (d : Durable) : Bool :=
  d.latest.any (fun a => a.guardianCode)

def Durable.latestCodeless (d : Durable) : Bool :=
  d.latest.any (fun a => !a.guardianCode)

/-- Volatile state owned by the one seat actor and its supervisor. -/
structure Volatile where
  consensusObserved : Bool
  /-- The supervisor's one-shot pre-first-spawn deletion intent. -/
  wipeFirst : Bool
  /-- A successfully spawned child exists and may answer a probe. -/
  childRunning : Bool
  deriving DecidableEq, Repr

/-- Externally relevant trace events, stored newest first. -/
inductive Event where
  | invite
  | wipe
  deriving DecidableEq, Repr

structure State where
  durable : Durable
  vol : Volatile
  /-- Emitted events, newest first. -/
  log : List Event
  deriving DecidableEq, Repr

/-- A consensus API may be the current child's state or stale state from the
prior attempt. Keeping the source explicit ensures A2 excludes both cases. -/
inductive ServingSource where
  | currentAttempt
  | priorAttempt
  deriving DecidableEq, Repr

/-- Probe results are otherwise unconstrained choices of the environment. -/
inductive Probe where
  | consensus (source : ServingSource)
  | setup
  | unreachable
  deriving DecidableEq, Repr

def Probe.isConsensus : Probe → Bool
  | .consensus _ => true
  | _ => false

/-- **Fedimintd hypothesis A2, made precise.** If a probe reports a consensus
API serving — whether attributed to the current attempt or the stopped prior
attempt — a child has completed a supervisor spawn and the current durable
attempt has a guardian code. Equivalently, no consensus result is admissible
when the current attempt is codeless (or absent).

The model does not prove this child/lifecycle contract. Every reachability and
headline theorem is parameterised by a caller-supplied proof of it. -/
def FedimintdA2 (st : State) (p : Probe) : Prop :=
  p.isConsensus = true → st.vol.childRunning = true ∧ st.durable.latestHasCode = true

/-- Primitive boundaries inside an actor command or supervisor launch. -/
inductive Effect where
  | appendCodeless
  | recordGuardianCode
  | recordConsensus
  | mirrorConsensus
  | setWipeFirst (value : Bool)
  | setChildRunning (value : Bool)
  | emit (event : Event)
  deriving DecidableEq, Repr

def Effect.apply (st : State) : Effect → State
  | .appendCodeless =>
      { st with durable := { st.durable with attempts := ⟨false⟩ :: st.durable.attempts } }
  | .recordGuardianCode =>
      match st.durable.attempts with
      | [] => st
      | _ :: tail =>
          { st with durable := { st.durable with attempts := ⟨true⟩ :: tail } }
  | .recordConsensus =>
      { st with durable := { st.durable with consensusObserved := true } }
  | .mirrorConsensus =>
      { st with vol := { st.vol with consensusObserved := true } }
  | .setWipeFirst value => { st with vol := { st.vol with wipeFirst := value } }
  | .setChildRunning value => { st with vol := { st.vol with childRunning := value } }
  | .emit event => { st with log := event :: st.log }

def runEffects (st : State) (effects : List Effect) : State :=
  effects.foldl Effect.apply st

/-- Startup reloads the permanent mirror and derives W1 solely from the latest
durable attempt. No pre-crash child is assumed to survive the daemon restart. -/
def State.reboot (st : State) : State :=
  { st with vol :=
      { consensusObserved := st.durable.consensusObserved
        wipeFirst := st.durable.latestCodeless
        childRunning := false } }

/-- First setup appends a row before recording its guardian code. A codeless
prefix is therefore durable wipe intent after a crash. -/
def dkgCodePlan : List Effect := [.appendCodeless, .recordGuardianCode]

/-- Both FI invite exits: observe durably, update the owned mirror, then let the
invite leave the process. Prefix truncation also represents fetch/transport
failure; such a prefix emits no invite. -/
def invitePlan : List Effect :=
  [.recordConsensus, .mirrorConsensus, .emit .invite]

/-- W2. The caller selects a probe, but a consensus result only records and
refuses. Setup/unreachable stops the old child, appends wipe intent, wipes, and
spawns with `wipe_first = false`. -/
def restartPlan (st : State) (probe : Probe) : List Effect :=
  if st.vol.consensusObserved then []
  else match probe with
    | .consensus _ => [.recordConsensus, .mirrorConsensus]
    | .setup | .unreachable =>
        [.setChildRunning false, .appendCodeless, .emit .wipe,
         .setWipeFirst false, .setChildRunning true]

/-- W1. The deletion flag is consumed before the first child can serve. A
normal respawn takes the false branch and preserves the directory. -/
def supervisorSpawnPlan (st : State) : List Effect :=
  if st.vol.wipeFirst then
    [.emit .wipe, .setWipeFirst false, .setChildRunning true]
  else [.setChildRunning true]

def Initial : State :=
  { durable := ⟨[], false⟩
    vol := ⟨false, false, false⟩
    log := [] }

/-- The transition relation is indexed by explicit A2 evidence. Actor
commands are indivisible with respect to other commands. Crash constructors are
the observationally distinct prefixes of their effect plans; await boundaries
that have emitted the same durable facts and events collapse to one prefix. -/
inductive Step (allowed : State → Probe → Prop) : State → State → Prop where
  | dkgCode (hnew : st.durable.attempts = []) (hrunning : st.vol.childRunning = true) :
      Step allowed st (runEffects st dkgCodePlan)
  /-- Crash after the first attempt row commits but before its code commits. -/
  | dkgCodeCrash (hnew : st.durable.attempts = []) (hrunning : st.vol.childRunning = true) :
      Step allowed st (runEffects st [.appendCodeless]).reboot
  | finishCode (hcodeless : st.durable.latestCodeless = true)
      (hrunning : st.vol.childRunning = true) :
      Step allowed st (Effect.recordGuardianCode.apply st)
  | invite (source : ServingSource) (hprobe : allowed st (.consensus source)) :
      Step allowed st (runEffects st invitePlan)
  /-- The invite *fetch* failed after the observation was recorded and mirrored
  (`seat.rs:999-1002`, and the `.ok()` in `report`). Not a crash: the child is
  untouched, the loop stays alive, and no invite leaves. Distinct from
  `inviteCrash` precisely because it must not reboot. -/
  | inviteFetchFailed (source : ServingSource) (hprobe : allowed st (.consensus source)) :
      Step allowed st (runEffects st [.recordConsensus, .mirrorConsensus])
  /-- Crash after the durable observation but before invite emission. A crash
  after the mirror update has the same rebooted state. -/
  | inviteCrash (source : ServingSource) (hprobe : allowed st (.consensus source)) :
      Step allowed st (runEffects st [.recordConsensus]).reboot
  | restart (probe : Probe) (hprobe : allowed st probe) :
      Step allowed st (runEffects st (restartPlan st probe))
  /-- Non-consensus restart prefix after stop+append but before W2. -/
  | restartCrashAfterAppend (probe : Probe) (hnon : probe.isConsensus = false)
      (hguard : st.vol.consensusObserved = false) :
      Step allowed st
        (runEffects st [.setChildRunning false, .appendCodeless]).reboot
  /-- W2's `remove_dir_all` failed (`seat.rs:920-928`). The code returns `Err`
  with the append committed, the old supervisor stopped and **no supervisor
  spawned**, and the next `restart_dkg` retries. This is not a crash and must not
  reboot: rebooting would re-derive `wipeFirst := true` from the codeless row,
  while the code has no supervisor that could act on it. -/
  | restartWipeFailed (probe : Probe) (hnon : probe.isConsensus = false)
      (hguard : st.vol.consensusObserved = false) :
      Step allowed st
        (runEffects st [.setChildRunning false, .appendCodeless, .setWipeFirst false])
  /-- Non-consensus restart prefix after W2. Later pre-reboot effects do not
  change durable facts or the log and therefore collapse to this state. -/
  | restartCrashAfterWipe (probe : Probe) (hnon : probe.isConsensus = false)
      (hguard : st.vol.consensusObserved = false) :
      Step allowed st
        (runEffects st [.setChildRunning false, .appendCodeless, .emit .wipe]).reboot
  | supervisorSpawn : Step allowed st (runEffects st (supervisorSpawnPlan st))
  /-- `SeatProcess::start` failed after W1 consumed the flag
  (`supervisor.rs:187-190`). The supervisor loop retries, but the directory is
  already gone and the flag is already clear, so no second wipe follows. Not a
  crash: no reboot, and `childRunning` stays false. -/
  | supervisorSpawnFailed (hpending : st.vol.wipeFirst = true) :
      Step allowed st (runEffects st [.emit .wipe, .setWipeFirst false])
  /-- Crash after W1 and before/after consuming the in-task flag. Startup
  re-derives intent from the still-codeless durable row. -/
  | supervisorSpawnCrashAfterWipe (hpending : st.vol.wipeFirst = true) :
      Step allowed st (runEffects st [.emit .wipe]).reboot
  | childCrash : Step allowed st (Effect.apply st (.setChildRunning false))
  | daemonCrash : Step allowed st st.reboot
  | other : Step allowed st st

inductive Reachable (allowed : State → Probe → Prop) : State → Prop where
  | init : Reachable allowed Initial
  | step {s s' : State} : Reachable allowed s → Step allowed s s' → Reachable allowed s'

end FMan.GuardianDataLoss
