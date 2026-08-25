# Current argument

## Argument

**L1 (`enum` + `code`) — normal privileged routes share one gate.** `app` builds
the complete normal `/admin` route table in `protected` and applies
`require_auth` once to that router. The only unprotected normal route is
`GET /health` ([`admin.rs`](../src/admin.rs)).

**L2 (`code`) — the gate fails closed.** `require_auth_token` accepts only an
exact `Bearer ` value of equal length through constant-time comparison; absent,
malformed, or unequal values return 401 without `next.run`.

**L3 (`enum` + `code`) — restore privileged routes share the same gate.**
`restore_app` places its complete protected route set (`/admin/health`,
`get_health`, `inspect_backup`, and `restore_backup`) behind
`require_restore_auth`; every state/value/secret/archive effect in that set
therefore first takes L2's gate ([`admin.rs`](../src/admin.rs)).

## Residual windows

## Weakest links

1. **L1/L3 (`enum`/`code`)** — future routes must stay inside an auth gate.
2. **L2 (`code`)** — header parsing and middleware short circuit.
3. **A1–A2 (`axiom`)** — Axum and bearer secrecy.
