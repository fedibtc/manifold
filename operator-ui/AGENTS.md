# operator-ui — agent router

Portable entry point for any agent (Codex, Claude Code, or a human driving one).
It routes work. It does not restate the coding rules.

**pnpm workspace** inside the `decentralized-federations` monorepo.
Apps: `apps/fleet-manager`, `apps/liquidity-provider`. Shared code: `packages/*`.

## Read before changing code

| Source | Owns |
|---|---|
| [`CLAUDE.md`](./CLAUDE.md) | component, style, and test conventions — authoritative |
| [`../AGENTS.md`](../AGENTS.md) | monorepo glossary and spec conventions |

Do not copy rules out of those files into this one.

## Pick the flow with the router

Do not guess which pipeline a request needs. Ask the router:

```bash
node /Users/kc/Projects/agent-harness/dist/cli/index.js route --explain "<what you want done>"
```

It reads the request, classifies intent and scope, and prints both the decision and
the equivalent `run` command. Drop `--explain` to execute the decision.

The harness CLI is not installed globally. From a checkout of `agent-harness`,
`pnpm dev route "<request>"` is the same command.

```
request ──► route ──► classify (kind, scope) ──► decide flags ──► run
                          │
                          └─► question / not a git repo ──► decline, nothing runs
```

`route` publishes only when you pass `--issue`. Every other path stops at a local
diff, so a human reviews before anything leaves the machine.

## Configuration layers

Three config files sit at this root. They are separate layers, not duplicates.

| File | Layer | Read by |
|---|---|---|
| `agent-toolkit.json` | lint + write guard | standards-hooks plugin, `eslint.config.mjs`, harness `src/config/load.ts` |
| `harness.config.json` | interactive Claude Code | `.claude/hooks/*.sh`, `scripts/gate.sh`, `scripts/review.sh`, `scripts/ratchet.mjs` |
| `agent-harness.json` | headless pipeline | the `agent-harness` CLI |

`agent-toolkit.json` is the **single source** for `protectedDirs`. Do not copy that
list anywhere else.

## Checks

`scripts/harness-gate.sh <stage>` runs one stage; `all` runs them in order:

`install · typecheck · lint · boundaries · style · structure · fallow · test`

Run evidence lands in `.agent-runs/` and worktrees in `.agent-worktrees/`. Both are
git-ignored. Application source stays in `apps/` and `packages/`.

## Human approval required

- Pushing a branch, opening a PR, or any publish step.
- Editing anything under `protectedDirs` — extend with a new file instead.
- Changing a config layer above, or a hook in `.claude/hooks/`.
