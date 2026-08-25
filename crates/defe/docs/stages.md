# Defe prospective work

Defe has no active staged implementation plan in this document. Track new work
in the project planning system rather than adding completion history here.

## Remaining static analysis

Run the following focused lint check and resolve any findings:

```bash
cargo clippy -p defe --all-targets -- -D warnings
```

## Deferred coverage

A real bitcoind lifecycle probe can add process-level coverage when needed. It
must remain ignored or use an explicit opt-in environment variable so ordinary
workspace tests do not require a `bitcoind` binary.

## Potential CLI addition

If Defe adds a print-and-exit resource command, it must state that its resources
are released as soon as the command exits.
