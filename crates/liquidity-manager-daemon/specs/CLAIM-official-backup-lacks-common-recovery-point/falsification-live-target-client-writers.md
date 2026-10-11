# Live target-client writers bypass the backup barrier

At source change `moqkvurn`, official backup acquires `WorkQuiescence`, takes a
SQLite snapshot, and copies non-SQLite payload files individually. The barrier
gates periodic worker passes, but it does not close retained target clients,
join their background state machines, constrain detached target-client opens, or
cover Admin inspection that can open a client.

Schedule a normal target-client durable write while backup enumerates and copies
its mutable RocksDB files. The filesystem premise permits the staging copy to
combine file contents from different instants, including a manifest which
references a file absent from the earlier enumeration. Compression and hashing
preserve that mixed copy rather than turn it into a checkpoint. The official
archive can therefore lack a target-store recovery instant and hence a common
SQLite/target-client recovery point. This is a source-derived consistency
counterexample; no runtime archive reproducer was added.
