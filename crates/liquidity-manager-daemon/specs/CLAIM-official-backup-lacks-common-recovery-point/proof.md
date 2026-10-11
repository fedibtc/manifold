# Current argument

## Argument

`WorkQuiescence` orders periodic worker passes against backup, and `VACUUM INTO`
provides one SQLite snapshot. Target-client background state machines, detached
client opens, and Admin-triggered opens are not covered, and backup copies their
mutable RocksDB files individually rather than taking a target-store checkpoint.
The source-derived counterexample is recorded in
[`falsification-live-target-client-writers.md`](falsification-live-target-client-writers.md).

## Weakest links

A common point requires quiescing/joining every target-client writer or using a
store-native checkpoint coordinated with the SQLite snapshot.
