# Current argument for CLAIM-flip-stability-provider-independent

## Scope

This implementation-grounded argument assesses whether the production
configuration and stability funding path represent the stability provider as an
actor independent from FLIP. It does not decide who legally owns credentials or
capital.

## Model

Independent representation requires four technical bindings: a distinct
stability-provider identity, its authenticated per-allocation authority, its
separate principal source, and provider-account control not derived from FLIP's
managed target client. Merely allowing a deployment operator to receive or
delegate credentials does not establish any of them.

## Argument

1. **`code` — one configured gatewayd funds every source.**
   `ARCH-liquidity-manager` and the daemon setup model identify one configured
   gatewayd-backed funding wallet for gateway and stability allocations.
   `stability_allocation` reaches the shared allocation-funding withdrawal path;
   no separate stability-provider wallet or payer appears in the request or
   setup configuration.
2. **`code` — FLIP creates the target-client authority.**
   `target_fedimint::load_or_generate_mnemonic` generates a per-federation
   mnemonic when the FLIP-managed target database is first joined, stores it in
   that database, and uses its root secret to build the client.
3. **`code` — that client derives the provider account.**
   `stability_pool::report` obtains
   `our_account(AccountType::Provider)` from the same target client, and the
   submission path calls that client's
   `deposit_to_provide_with_operation_id`. No independently supplied
   stability-provider account or signing key is accepted.
4. **`schema + API` — the advertised provider identity is not the missing
   binding.** Public allocation payloads name FLIP's provider service identity,
   while the target provider account derives from the target-client root secret.
   The production API and schema carry no distinct stability-provider identity,
   delegation, beneficiary, or per-allocation authorization connecting those
   authorities.

Thus the current implementation can be operated as an agent with delegated
credentials, but it does not represent or enforce an independent stability
provider. It technically permits the FLIP deployment to source principal from
its configured wallet and control the provider account through its managed
target-client secret, contrary to the claim.

## Residuals

- Credential possession does not establish that the daemon operator is the
  legal or beneficial owner.
- An external agreement may delegate wallet and client authority to the
  operator, but no supported configuration field or protocol binding represents
  that agreement.

## Weakest links

1. “Entity” is not a source-level concept; this argument verifies represented
   identities, authorities, and control rather than legal personhood.
2. A future separate funding backend, external account key, or delegation record
   would change the implementation boundary and require re-verification.
