# Fedimint gateway daemon resource

`Gatewayd` is a local Fedimint gateway daemon resource. Its request carries a
leased bitcoind descriptor and optional direct Iroh routes; those are
non-owning launch dependencies, so the requester keeps their leases alive.

Gatewayd may be shared. Defe reuses a slot only when the complete launch
request matches, including its bitcoind endpoint and Iroh overrides. The
descriptor returns the administrative API URL and credential. Consumers that
need isolated gateway wallet state request an exclusive lease instead.
