# Proof: Fleet Manager production deployment envelope

## Scope and model

This is a compositional conditional argument for
[CLAIM-fleet-manager-production-deployment-envelope](../CLAIM-fleet-manager-production-deployment-envelope.md).
It names the external host/operator premises under which FMan software is
assessed. It neither inspects nor certifies an Umbrel, StartOS, VPS, container,
or other actual deployment. The changed premise set leaves the claim
**Unverified**; this maintenance records no fresh verification result.

## Assumption boundary

The assumptions directly supply one-instance topology; operator custody of the
admin socket, data root, credentials, and backups; capacity; public-ingress
abuse controls; and health monitoring. The reviewed FMan artifact and its
bundled, pinned `fedimintd` are one TCB: the child process boundary manages
lifetime and crashes, not malicious-child containment. The linked checklist is
an operator obligation to record evidence for those premises, not a program that
can establish them.

## Argument

1. **[assumption] External envelope.** Each topology, custody, capacity,
   ingress, and monitoring clause is supplied by its same-named operator premise.
2. **[assumption] Artifact integrity.** The reviewed FMan release and bundled,
   pinned `fedimintd` supply the trusted software boundary. A changed child is
   outside this implication, rather than contained by the process model.
3. **[logic] Joint sufficiency.** The deliberate completeness challenge assigned
   every property clause to one of these premises. No premise says that a live
   platform has actually supplied the control.

## Residuals

A deployment that lacks an item may fail in practice, but does not contradict
this conditional software property. The checklist is not a certification,
attestation, startup gate, or substitute for platform qualification.
