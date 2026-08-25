# Current counterexample

The endorsement authenticates only the federation id. An attacker can retain
that id in a syntactically valid invite, replace its API URL, and make FLIP dial
the chosen endpoint before FLIP authenticates the federation configuration
returned by it.

`GlobalOnly` closes loopback, link-local, transition-address, redirect, and DNS
re-resolution variants, and requester-visible preview errors are sanitized.
Those controls limit the reachable endpoint set and returned information; they
do not authenticate the invite URL before the initial dial.

See [the current argument](proof.md) for the preserved premises, residuals, and detailed derivation.
