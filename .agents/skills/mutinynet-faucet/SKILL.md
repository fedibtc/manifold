---
name: mutinynet-faucet
description: Run and use the persistent local Mutinynet treasury to receive test sats, fund on-chain addresses, pay Lightning invoices, or perform gateway wallet and ecash operations. Use for staging workflows that need Mutinynet money, including FI and FLIP testing.
---

# Mutinynet faucet wallet

Use this skill when an agent or user needs test money for a staging workflow.
This is a persistent Mutinynet treasury, not Defe's ephemeral Regtest funding.

## Start the treasury

Run this in a dedicated terminal and leave it running:

```bash
just mutinynet-faucet
```

The wallet persists under
`${MUTINYNET_FAUCET_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/manifold/mutinynet-faucet}`.
Stopping the process does not discard its keys or balance.

## Ask the user to fund it

Once the daemon is ready, always obtain a fresh address rather than copying one
from old output:

```bash
just mutinynet-wallet onchain address
```

Read the `address` field from the JSON response and give that exact address to
the user. Tell them it is **Mutinynet test money only** and they can fund it at
<https://faucet.mutinynet.com/>. Do not claim the wallet is funded until the
balance command confirms it:

```bash
just mutinynet-wallet get-balances
```

## Spend from the treasury

Fund another Mutinynet on-chain address:

```bash
just mutinynet-fund <ADDRESS> <SATS>
```

Top up a configured staging FLIP instance without copying its deposit address:

```bash
just staging-flip-top-up <SATS> [--instance N]
```

This asks FLIP to create a tracked gateway deposit address, then funds that
address from the treasury. Prefer it over sending directly to an arbitrary
address when testing FLIP, because FLIP can then observe the incoming operation.

Pay a Mutinynet Lightning invoice:

```bash
just mutinynet-pay <INVOICE>
```

After the on-chain wallet is funded, optionally open a channel to Mutinynet's
public faucet node:

```bash
just mutinynet-open-channel <SATS>
```

For balances, receiving addresses, invoices, federation joins, and ecash
operations, pass the corresponding gateway CLI command through directly:

```bash
just mutinynet-wallet <gateway-cli command...>
```

Use `gateway-cli <command> --help` to confirm argument and amount units before
an irreversible send. Never expose the treasury API beyond loopback, print its
stored password, use it with mainnet funds, or confuse it with FLIP's separate
accounting wallet.
