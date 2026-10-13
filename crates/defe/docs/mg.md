# Manifold Guardian resource

`Mg` is a local `fleet-manager` resource. Federation formation mutates a
manager's seat state, so formation tests request exclusive leases even though a
manager is substantially cheaper to create than the federation it may form.

`MgRequest` carries only topology that Defe cannot infer for an independent
manager: the leased bitcoind and relay endpoints and the coordinated local
Iroh route/seat-port grid. Those endpoint descriptors are non-owning launch
dependencies; the requesting client keeps their leases alive until it releases
the MG.

Defe owns the MG process, its slot data directory, logs, default one-seat
configuration, and lifecycle. `MgInfo` returns the FA locator and stable
data directory. A request may use `shared` when its complete topology matches
an existing slot; otherwise it uses `exclusive`. A full local Iroh grid requires
an explicit aggregate resource; independent MG requests do not guess one.
