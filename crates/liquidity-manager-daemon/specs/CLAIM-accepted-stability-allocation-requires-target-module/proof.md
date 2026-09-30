# Current argument

## Argument

Admission compares the requested federation/config/network with the authenticated
preview and rejects a positive stability allocation unless the preview's module
kinds contain `multi_sig_stability_pool`. Before allocating a peg-in address, the
worker independently checks the exact config hash and resolves an actual
`StabilityPoolClientModule`. The historical ordinary-moduleless funding trace is
therefore blocked both before acceptance and before outflow.

## Unresolved proof obligations

The admission check establishes the authenticated module-kind string, not that
every admitted version and configuration byte sequence can instantiate a usable
client module. The later worker check does not prove the claim's stronger
“verifies usable before acceptance” wording.

## Weakest links

Passing requires either a proof that every supported authenticated kind is usable
or an admission-time usability check.
