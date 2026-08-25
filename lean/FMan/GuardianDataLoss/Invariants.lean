import FMan.GuardianDataLoss.Model

/-!
# Guardian data-loss invariants

The invariant separates durable/log facts from volatile actor ownership. Crash
constructors enumerate the observably distinct prefixes of each short effect
plan; boundaries with identical durable facts and events are intentionally
collapsed.
-/

namespace FMan.GuardianDataLoss

/-- No wipe occurs chronologically after an invite in a newest-first log. -/
def LogSafe (log : List Event) : Prop :=
  ∀ pre post, log = pre ++ Event.invite :: post → Event.wipe ∉ pre

structure Base (st : State) : Prop where
  observedCoded : st.durable.consensusObserved = true → st.durable.latestHasCode = true
  inviteObserved : Event.invite ∈ st.log → st.durable.consensusObserved = true
  logSafe : LogSafe st.log

structure Inv (st : State) : Prop extends Base st where
  mirror : st.vol.consensusObserved = st.durable.consensusObserved
  pendingCodeless : st.vol.wipeFirst = true → st.durable.latestCodeless = true
  /-- A child cannot serve until W1 has been consumed. -/
  pendingStopped : st.vol.wipeFirst = true → st.vol.childRunning = false

theorem latest_exclusive {d : Durable} (hc : d.latestCodeless = true) :
    d.latestHasCode = false := by
  cases d with
  | mk attempts observed =>
    cases attempts with
    | nil => simp [Durable.latestCodeless, Durable.latest] at hc
    | cons a tail =>
      cases a with
      | mk code => cases code <;>
          simp [Durable.latestCodeless, Durable.latestHasCode, Durable.latest] at *

theorem no_invite_of_unobserved {st : State} (hb : Base st)
    (hobs : st.durable.consensusObserved = false) : Event.invite ∉ st.log := by
  intro hin
  rw [hb.inviteObserved hin] at hobs
  contradiction

theorem logSafe_cons_invite {log : List Event} (hs : LogSafe log) :
    LogSafe (Event.invite :: log) := by
  intro pre post heq
  cases pre with
  | nil => simp
  | cons x xs =>
    simp only [List.cons_append, List.cons.injEq] at heq
    obtain ⟨rfl, heq⟩ := heq
    simp only [List.mem_cons, not_or]
    exact ⟨by decide, hs xs post heq⟩

theorem logSafe_cons_wipe {log : List Event} (hno : Event.invite ∉ log) : LogSafe (Event.wipe :: log) := by
  intro pre post heq
  cases pre with
  | nil => simp
  | cons x xs =>
    simp only [List.cons_append, List.cons.injEq] at heq
    obtain ⟨rfl, heq⟩ := heq
    exfalso
    apply hno
    rw [heq]
    simp

theorem false_of_true_eq_false {b : Bool} (ht : b = true) (hf : b = false) : False := by
  rw [ht] at hf
  contradiction

theorem bool_eq_false {b : Bool} (h : b = true → False) : b = false := by
  cases b with
  | false => rfl
  | true => exact False.elim (h rfl)

theorem inv_reboot {st : State} (hb : Base st) : Inv st.reboot := by
  refine { toBase := ?_, mirror := rfl, pendingCodeless := ?_, pendingStopped := ?_ }
  · exact Base.mk hb.observedCoded hb.inviteObserved hb.logSafe
  · intro h
    simpa [State.reboot] using h
  · intro _
    rfl

theorem inv_step {allowed : State → Probe → Prop}
    (a2 : ∀ st p, allowed st p → FedimintdA2 st p) {s s' : State}
    (hi : Inv s) (step : Step allowed s s') : Inv s' := by
  cases step with
  | dkgCode hnew hrunning =>
    have hd : s.durable.consensusObserved = false :=
      bool_eq_false (fun ht => by
        have hc := hi.observedCoded ht
        simp [Durable.latestHasCode, Durable.latest, hnew] at hc)
    have hp : s.vol.wipeFirst = false :=
      bool_eq_false (fun ht =>
        false_of_true_eq_false hrunning (hi.pendingStopped ht))
    refine
      { observedCoded := by intro ho; simp [dkgCodePlan, runEffects, Effect.apply, hd] at ho
        inviteObserved := by simpa [dkgCodePlan, runEffects, Effect.apply] using hi.inviteObserved
        logSafe := by simpa [dkgCodePlan, runEffects, Effect.apply] using hi.logSafe
        mirror := by simpa [dkgCodePlan, runEffects, Effect.apply, hd] using hi.mirror
        pendingCodeless := by simp [dkgCodePlan, runEffects, Effect.apply, hp]
        pendingStopped := by simp [dkgCodePlan, runEffects, Effect.apply, hp] }
  | dkgCodeCrash hnew hrunning =>
    apply inv_reboot
    refine
      { observedCoded := ?_
        inviteObserved := by simpa [runEffects, Effect.apply] using hi.inviteObserved
        logSafe := by simpa [runEffects, Effect.apply] using hi.logSafe }
    intro ho
    have hc := hi.observedCoded ho
    simp [Durable.latestHasCode, Durable.latest, hnew] at hc
  | finishCode hcodeless hrunning =>
    cases ha : s.durable.attempts with
    | nil => simp [Durable.latestCodeless, Durable.latest, ha] at hcodeless
    | cons a tail =>
     have hp : s.vol.wipeFirst = false :=
       bool_eq_false (fun ht =>
         false_of_true_eq_false hrunning (hi.pendingStopped ht))
     refine
       { observedCoded := by intro _; simp [Effect.apply, ha, Durable.latestHasCode, Durable.latest]
         inviteObserved := by simpa [Effect.apply, ha] using hi.inviteObserved
         logSafe := by simpa [Effect.apply, ha] using hi.logSafe
         mirror := by simpa [Effect.apply, ha] using hi.mirror
         pendingCodeless := by simp [Effect.apply, ha, hp]
         pendingStopped := by simp [Effect.apply, ha, hp] }
  | invite source hprobe =>
    have hcode := (a2 _ _ hprobe rfl).2
    have hp : s.vol.wipeFirst = false :=
      bool_eq_false (fun ht =>
        false_of_true_eq_false hcode (latest_exclusive (hi.pendingCodeless ht)))
    refine
      { observedCoded := by intro _; simpa [invitePlan, runEffects, Effect.apply] using hcode
        inviteObserved := by simp [invitePlan, runEffects, Effect.apply]
        logSafe := by simpa [invitePlan, runEffects, Effect.apply] using
          logSafe_cons_invite hi.logSafe
        mirror := by simp [invitePlan, runEffects, Effect.apply]
        pendingCodeless := by simp [invitePlan, runEffects, Effect.apply, hp]
        pendingStopped := by simp [invitePlan, runEffects, Effect.apply, hp] }
  | inviteFetchFailed source hprobe =>
    have hcode := (a2 _ _ hprobe rfl).2
    refine
      { observedCoded := by intro _; simpa [runEffects, Effect.apply] using hcode
        inviteObserved := by simp [runEffects, Effect.apply]
        logSafe := by simpa [runEffects, Effect.apply] using hi.logSafe
        mirror := by simp [runEffects, Effect.apply]
        pendingCodeless := by simpa [runEffects, Effect.apply] using hi.pendingCodeless
        pendingStopped := by simpa [runEffects, Effect.apply] using hi.pendingStopped }
  | inviteCrash source hprobe =>
    have hcode := (a2 _ _ hprobe rfl).2
    apply inv_reboot
    refine
      { observedCoded := by intro _; simpa [runEffects, Effect.apply] using hcode
        inviteObserved := by simp [runEffects, Effect.apply]
        logSafe := by simpa [runEffects, Effect.apply] using hi.logSafe }
  | restart probe hprobe =>
    by_cases hm : s.vol.consensusObserved = true
    · simp [restartPlan, hm, runEffects]
      exact hi
    · have hd : s.durable.consensusObserved = false := by
        rw [← hi.mirror]
        exact bool_eq_false hm
      cases probe with
      | consensus source =>
        have hcode := (a2 _ _ hprobe rfl).2
        have hp : s.vol.wipeFirst = false :=
          bool_eq_false (fun ht =>
            false_of_true_eq_false hcode (latest_exclusive (hi.pendingCodeless ht)))
        refine
          { observedCoded := by intro _; simpa [restartPlan, hm, runEffects, Effect.apply] using hcode
            inviteObserved := by simp [restartPlan, hm, runEffects, Effect.apply]
            logSafe := by simpa [restartPlan, hm, runEffects, Effect.apply] using hi.logSafe
            mirror := by simp [restartPlan, hm, runEffects, Effect.apply]
            pendingCodeless := by simp [restartPlan, hm, runEffects, Effect.apply, hp]
            pendingStopped := by simp [restartPlan, hm, runEffects, Effect.apply, hp] }
      | setup =>
        have hno := no_invite_of_unobserved hi.toBase hd
        refine
          { observedCoded := by intro ho; simp [restartPlan, hm, runEffects, Effect.apply, hd] at ho
            inviteObserved := by simpa [restartPlan, hm, runEffects, Effect.apply] using hi.inviteObserved
            logSafe := by simpa [restartPlan, hm, runEffects, Effect.apply] using
              logSafe_cons_wipe hno
            mirror := by simp [restartPlan, hm, runEffects, Effect.apply, hd]
            pendingCodeless := by simp [restartPlan, hm, runEffects, Effect.apply]
            pendingStopped := by simp [restartPlan, hm, runEffects, Effect.apply] }
      | unreachable =>
        have hno := no_invite_of_unobserved hi.toBase hd
        refine
          { observedCoded := by intro ho; simp [restartPlan, hm, runEffects, Effect.apply, hd] at ho
            inviteObserved := by simpa [restartPlan, hm, runEffects, Effect.apply] using hi.inviteObserved
            logSafe := by simpa [restartPlan, hm, runEffects, Effect.apply] using
              logSafe_cons_wipe hno
            mirror := by simp [restartPlan, hm, runEffects, Effect.apply, hd]
            pendingCodeless := by simp [restartPlan, hm, runEffects, Effect.apply]
            pendingStopped := by simp [restartPlan, hm, runEffects, Effect.apply] }
  | restartCrashAfterAppend probe hnon hguard =>
    have hd : s.durable.consensusObserved = false := by
      rw [← hi.mirror]
      exact hguard
    apply inv_reboot
    refine
      { observedCoded := by simp [runEffects, Effect.apply, hd]
        inviteObserved := by simpa [runEffects, Effect.apply] using hi.inviteObserved
        logSafe := by simpa [runEffects, Effect.apply] using hi.logSafe }
  | restartWipeFailed probe hnon hguard =>
    have hd : s.durable.consensusObserved = false := by
      rw [← hi.mirror]
      exact hguard
    refine
      { observedCoded := by simp [runEffects, Effect.apply, hd]
        inviteObserved := by simpa [runEffects, Effect.apply] using hi.inviteObserved
        logSafe := by simpa [runEffects, Effect.apply] using hi.logSafe
        mirror := by simp [runEffects, Effect.apply, hd, hguard]
        pendingCodeless := by simp [runEffects, Effect.apply]
        pendingStopped := by simp [runEffects, Effect.apply] }
  | restartCrashAfterWipe probe hnon hguard =>
    have hd : s.durable.consensusObserved = false := by
      rw [← hi.mirror]
      exact hguard
    have hno := no_invite_of_unobserved hi.toBase hd
    apply inv_reboot
    refine
      { observedCoded := by simp [runEffects, Effect.apply, hd]
        inviteObserved := by simpa [runEffects, Effect.apply] using hi.inviteObserved
        logSafe := by simpa [runEffects, Effect.apply] using
          logSafe_cons_wipe hno }
  | supervisorSpawn =>
    by_cases hp : s.vol.wipeFirst = true
    · have hc := hi.pendingCodeless hp
      have hd : s.durable.consensusObserved = false :=
        bool_eq_false (fun ht =>
          false_of_true_eq_false (hi.observedCoded ht) (latest_exclusive hc))
      have hno := no_invite_of_unobserved hi.toBase hd
      refine
        { observedCoded := by simpa [supervisorSpawnPlan, hp, runEffects, Effect.apply] using hi.observedCoded
          inviteObserved := by simpa [supervisorSpawnPlan, hp, runEffects, Effect.apply] using hi.inviteObserved
          logSafe := by simpa [supervisorSpawnPlan, hp, runEffects, Effect.apply] using
            logSafe_cons_wipe hno
          mirror := by simpa [supervisorSpawnPlan, hp, runEffects, Effect.apply] using hi.mirror
          pendingCodeless := by simp [supervisorSpawnPlan, hp, runEffects, Effect.apply]
          pendingStopped := by simp [supervisorSpawnPlan, hp, runEffects, Effect.apply] }
    · have hp' := bool_eq_false hp
      refine
        { observedCoded := by simpa [supervisorSpawnPlan, hp', runEffects, Effect.apply] using hi.observedCoded
          inviteObserved := by simpa [supervisorSpawnPlan, hp', runEffects, Effect.apply] using hi.inviteObserved
          logSafe := by simpa [supervisorSpawnPlan, hp', runEffects, Effect.apply] using hi.logSafe
          mirror := by simpa [supervisorSpawnPlan, hp', runEffects, Effect.apply] using hi.mirror
          pendingCodeless := by simp [supervisorSpawnPlan, hp', runEffects, Effect.apply]
          pendingStopped := by simp [supervisorSpawnPlan, hp', runEffects, Effect.apply] }
  | supervisorSpawnFailed hpending =>
    have hc := hi.pendingCodeless hpending
    have hd : s.durable.consensusObserved = false :=
      bool_eq_false (fun ht =>
        false_of_true_eq_false (hi.observedCoded ht) (latest_exclusive hc))
    have hno := no_invite_of_unobserved hi.toBase hd
    refine
      { observedCoded := by simpa [runEffects, Effect.apply] using hi.observedCoded
        inviteObserved := by simpa [runEffects, Effect.apply] using hi.inviteObserved
        logSafe := by simpa [runEffects, Effect.apply] using logSafe_cons_wipe hno
        mirror := by simpa [runEffects, Effect.apply] using hi.mirror
        pendingCodeless := by simp [runEffects, Effect.apply]
        pendingStopped := by simp [runEffects, Effect.apply] }
  | supervisorSpawnCrashAfterWipe hpending =>
    have hc := hi.pendingCodeless hpending
    have hd : s.durable.consensusObserved = false :=
      bool_eq_false (fun ht =>
        false_of_true_eq_false (hi.observedCoded ht) (latest_exclusive hc))
    have hno := no_invite_of_unobserved hi.toBase hd
    apply inv_reboot
    refine
      { observedCoded := by simpa [runEffects, Effect.apply] using hi.observedCoded
        inviteObserved := by simpa [runEffects, Effect.apply] using hi.inviteObserved
        logSafe := by simpa [runEffects, Effect.apply] using
          logSafe_cons_wipe hno }
  | childCrash =>
    refine
      { observedCoded := by simpa [Effect.apply] using hi.observedCoded
        inviteObserved := by simpa [Effect.apply] using hi.inviteObserved
        logSafe := by simpa [Effect.apply] using hi.logSafe
        mirror := by simpa [Effect.apply] using hi.mirror
        pendingCodeless := by simpa [Effect.apply] using hi.pendingCodeless
        pendingStopped := by simp [Effect.apply] }
  | daemonCrash => exact inv_reboot hi.toBase
  | other => exact hi

theorem inv_initial : Inv Initial := by
  refine
    { observedCoded := by simp [Initial]
      inviteObserved := by simp [Initial]
      logSafe := by simp [Initial, LogSafe]
      mirror := rfl
      pendingCodeless := by simp [Initial]
      pendingStopped := by simp [Initial] }

theorem inv_reachable {allowed : State → Probe → Prop}
    (a2 : ∀ st p, allowed st p → FedimintdA2 st p) {st : State}
    (h : Reachable allowed st) : Inv st := by
  induction h with
  | init => exact inv_initial
  | step _ hs ih => exact inv_step a2 ih hs

end FMan.GuardianDataLoss
