# Guardian transfer QA harness (local defe)

This is the harness used to QA guardian transfer end to end on a local defe stack. It ran 14 real MG daemons and real bridge FAs (the fedi REPL harness on branch `rho/qa-gt-harness2` in fedi).

- `qaenv.py`: starts and onboards the 14 MGs, wires their ports through the UDP/TCP fault proxy, and publishes the setup-payment policy.
- `netproxy.py`: a UDP/TCP proxy with a control socket. It cuts MG endpoints, guardian ports, and Nostr relay groups for fault injection.
- `nostr_sign.py`: minimal Nostr event signing for the test publisher.
- `notebook_cells.py`: every notebook cell, in the order it ran. The scenario runners (`run_scenario`, `sweep2`, `sweep_priv`, `R4*`, `T3off`, `Edecom2`, and the routing monitor) are defined here. It is not a standalone script.

Known harness limits:
- MG port blocks are `base + 100*g`, with the MG endpoint at `base + 99`. Seat ordinals are never reused, so an MG that passes seat 21 collides with its own endpoint (through fedimintd's extra Iroh 1.0 socket at api+10) and, from seat 25, with the next MG's ports.
- The regtest bitcoind needs a block at least every couple of hours. Otherwise fedimintd waits for sync (progress below 0.999) and new guardians never start consensus.
