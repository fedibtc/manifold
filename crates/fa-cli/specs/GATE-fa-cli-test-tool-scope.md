# GATE-fa-cli-test-tool-scope: fa-cli is a developer test tool

## Gate

`fa-cli` is a development/test tool for developers. Fixing or defensively
handling hypothetical corner cases that do not arise in its actual
developer/E2E usage — for example concurrent `fa-cli` invocations or clock
jumps — is out of scope. A review finding against `fa-cli` is not accepted as
something to fix by default: it must first be adjudicated against this purpose
and scope, and findings that matter only under such hypothetical conditions
are rejected rather than fixed.

## Justification

Product-only edge cases do not occur in supported `fa-cli` developer/E2E
workflows. Handling them in the CLI would add complexity without improving its
test-tool purpose; those concerns belong in production consumers or `fa-client`
integration where applicable. The user wants review findings against `fa-cli`
weighed against its purpose as a thin test consumer of
[`fa-client`](../../fa-client/specs/ARCH-fa-client.md) instead of treated as
mandatory work. [ARCH-fa-cli](ARCH-fa-cli.md) records the development/test-only
role this gate protects.
