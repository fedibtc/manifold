# CLAIM-fleet-manager-production-deployment-envelope: Fleet Manager production deployment envelope

Given a secure external host/operator envelope, FMan software operates as one
instance for one tenant, restricts the admin
socket and data root to the operator, protects credential-bearing storage and
backups, supplies durable capacity before exhaustion, applies deployment-level
abuse controls to the public RPC surface, and monitors process, disk, child,
relay-publication, and wallet health. The reviewed FMan release and its
bundled, pinned `fedimintd` retain implementation integrity as one FMan TCB.

This is a conditional software property. It does not inspect, certify, or prove
an actual Umbrel, StartOS, VPS, container, or other deployment.

## Status

Unverified: the current property and external-envelope premise have not been verified.

## Assumptions

- Fleet Manager runs as one instance for one tenant.
- Only the operator can access the admin socket and data root.
- Credential-bearing storage and backups are protected.
- Durable capacity is supplied before exhaustion.
- Deployment-level abuse controls apply to the public RPC surface.
- Process, disk, child, relay-publication, and wallet health are monitored.
- The deployed artifact is the reviewed FMan release and its bundled, pinned
  `fedimintd`; both retain their implementation integrity as FMan TCB.
- The operator satisfies the discoverable
  [secure-deployment checklist](../../../packages/fleet-manager/secure-deployment.md).
