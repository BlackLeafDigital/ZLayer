# ZLayer Komodo-Parity TODO

See the primary copy at `../ZLayer/ZLAYER_TODO.md` for the full document. This mirror copy is kept in sync.

**This repo (zlayer-zql) uses ZQL, not sqlx/SQLite. sqlx and redb have been REMOVED from this workspace.**

## Quick summary of what needs fixing here (mirror-relevant only)

1. **zlayer-git** (`crates/zlayer-git/src/lib.rs`) — rewrite from git CLI to gix. Same changes as main repo.
2. **Persistent storage adapter** (`crates/zlayer-api/src/storage/persistent_adapter.rs`) — `ZqlJsonStore` wrapping `zql::Database` with `put_typed`/`get_typed`/`scan_typed`. Implements all storage traits generically. Wire in `bin/zlayer/src/commands/serve.rs` replacing InMemory stores.
3. **Workflow execution** (`crates/zlayer-api/src/handlers/workflows.rs`) — make BuildProject/DeployProject/ApplySync actions actually call real handlers. Same code as main repo.
4. **SMTP notifier** (`crates/zlayer-api/src/handlers/notifiers.rs`) — implement with `lettre`. Same code as main repo.
5. **Sync apply** (`crates/zlayer-api/src/handlers/syncs.rs`) — make apply actually create/update/delete resources. Same code as main repo.

**NOT mirrored**: Manager UI (fix 6). `zlayer-manager` is not in this workspace.
