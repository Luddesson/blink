# Archived Modules

The following modules were archived because they had no references in the active codebase and were not integrated into the runtime or RPC interfaces.

## Unreferenced Modules

These modules were exported from lib.rs but had zero references across the entire codebase:

- **buffer_pool** - Memory pooling utility (archived: no runtime usage)
- **bullpen_reconciler** - Reconciliation logic for bullpen integration (archived: not wired in agent RPC)
- **game_start_watcher** - In-play market detection (archived: no active consumers)
- **gas_strategy** - Gas optimization strategy (archived: no implementation found)
- **heartbeat** - Health check mechanism (archived: replaced by agent RPC health checks)
- **in_play_failsafe** - Safety switch for in-play markets (archived: redundant with risk_manager)
- **io_uring_net** - Network abstraction via io_uring (archived: not integrated)
- **market_metadata** - Market information cache (archived: pulled from live endpoints)
- **mev_shield** - MEV protection mechanism (archived: candidate for future work)
- **position_tracker** - Position tracking state (archived: replaced by paper_portfolio)

## Restoration

To restore any of these modules:
1. Uncomment the `pub mod module_name;` line in `crates/engine/src/lib.rs`
2. Ensure all dependencies are still available
3. Review the source code for recent changes (see git history)
4. Re-integrate into appropriate runtime or RPC interface
5. Run `scripts/generate-project-inventory.ps1` to update status

## Reasoning

Archiving vs. deletion preserves code history while clearly marking these as out-of-scope. The inventory system will mark them as `archived-or-legacy` until they're either restored or permanently deleted.
