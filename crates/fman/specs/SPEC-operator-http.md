# SPEC-operator-http: Browser operator API

## Record justification

The contract spans the HTTP adapter, owner-only admin socket, browser session
handling, and daemon deployment boundary, so no single implementation artifact
can own it coherently.

The FMan may expose an HTTP API for operator administration. The server is
disabled unless both a bind address and an authentication mode are configured.
Nix release builds also embed the operator dashboard in the binary and serve it
from this listener. Other builds serve only the API.

Static and SPA routing is supplied by the shared `operator-ui-static` crate;
the FMan binary owns only the selection of its dashboard assets and the `/api`
namespace reservation. The static router is merged outside the API's session
middleware. Consequently `/`, `/index.html`, and `/assets/*` are available
before password authentication, while the API routes retain their existing
authentication requirements.

The embedded dashboard is public within the listener's authentication boundary:
password mode must serve the login shell before a session exists, while trusted
proxy mode relies on the proxy protecting the complete listener. `GET` and
`HEAD` serve exact embedded files; extensionless browser-navigation paths fall
back to `index.html` for the single-page router. Missing `/api/*`, `/assets/*`,
and mock-control paths never fall back to HTML. `index.html` and non-hashed
files use `Cache-Control: no-store`; Vite's content-hashed `/assets/*` use
`Cache-Control: public, max-age=31536000, immutable`. Static responses include
their media type and `X-Content-Type-Options: nosniff`, and are gzipped when the
client accepts that encoding. No proxy sits between the dashboard and this
listener, so the daemon owns compression for its own assets.

`POST /api/admin` accepts the same serialized `AdminRequest` values as
[SPEC-admin-socket](./SPEC-admin-socket.md) and invokes the same in-process
dispatcher; HTTP is not a second set of operator semantics. Every response uses
`Cache-Control: no-store`, because responses may contain the mnemonic and
operator financial details. Request and response bodies must not be logged.

## The onboarding phase

This listener serves a host that has not been onboarded, because on this
listener is where the operator onboards it. Umbrel has no install-time user
input, and its own packaging guidance points at a browser setup flow after
install; StartOS could ask, but a recovery phrase typed into a platform config
form is a mnemonic at rest in that platform's store, which the dashboard —
holding it only in memory — avoids.

So the listener is bound for the complete durable onboarding workflow: identity
creation or restore, manual Holder-authorization refresh, and initial
price/capacity configuration. It serves the onboarding verbs of
[SPEC-admin-socket](./SPEC-admin-socket.md), the embedded dashboard that calls
them, and a refusal for every unrelated verb carrying
`AdminErrorKind::not_onboarded`. That discriminant is the browser's status
read: a host with no identity answers, so "not set up" is distinguishable from
"not answering" without inferring either from a connection failure or from the
refusal's prose.

The status projections — `Onboarding` and the holder refresh's answer —
include the durable stage, so a browser reload asks status and resumes at the
first unfinished step; transition verbs answer with their own outcome and the
client re-reads status for the cursor. Holder refresh is an awaited bounded
relay operation, not a scheduled background request followed by a racy read.
After the final write the phase reports `runtime: starting` until fleet open
completes.

**One phase switch, both transports.** When the fleet opens, the shared operator
phase switches both bound listeners to the full dispatcher in place. An
operator who has just answered the last question of the wizard must not have to
find a port or socket that went away and came back. Between the answer and the
open fleet the phase reports neither the running answers nor `not_onboarded`,
which would send that operator back to the wizard's first screen for a host that
is already set up.

The authentication boundary does not move with the phase. The password is read
from a file the deployment wrote, not derived from the identity, so password
mode protects onboarding exactly as it protects the fleet: an operator signs in,
then sets the host up. Trusted-proxy mode is unchanged — the proxy protects the
complete listener in both phases.

The Unix socket serves the same phase at the same time, through the same
implementation. Onboarding happens once whichever transport carries it, and the
daemon continues into its fleet on the identity that transport produced.

## Authentication modes

The deployment selects exactly one mode:

- **Trusted proxy.** FMan performs no local authentication. This mode is valid
  only when the listener has no host port and an authenticating platform proxy
  is its sole network peer. The initial target is Umbrel's authenticated app
  proxy on an app-private container network.
- **Password.** The deployment generates an operator password and writes it to a
  file readable by the FMan process. A client submits it as JSON to
  `POST /api/auth`. FMan verifies it using constant-time comparison and returns
  an opaque random, HTTP-only, same-site session cookie. Invalid credentials
  return `401 Unauthorized`; success returns `204 No Content`. Session secrets
  are in memory, so a daemon restart invalidates every session. StartOS and
  bare-host HTTP exposure require this mode unless another authenticated proxy
  is explicitly configured.

The password file is forbidden in trusted-proxy mode. Password mode fails
startup if the file is absent, empty, or larger than 1024 bytes. The
authentication endpoint is public; a Tower layer protects the complete admin
API router so new handlers do not silently omit authentication. Unauthenticated
admin requests return `401 Unauthorized`.

The Unix socket remains enabled and remains the local CLI and recovery
interface. HTTP does not weaken or replace its filesystem ownership boundary.

Constrained by [REQ-no-public-ip](./REQ-no-public-ip.md) and
[ARCH-fleet-manager-identity](./ARCH-fleet-manager-identity.md).
