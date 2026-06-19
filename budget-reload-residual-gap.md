# Residual gap after #8004 — enabling cost tracking on reload still needs a restart

**Context:** Follow-up to PR #8004 (`fix(cost): make budget config reloadable instead of frozen at boot`).
#8004 fixes the case that caused our incident (changing an *already-enabled* budget limit now
takes on reload). This note is about the one case it deliberately leaves unfixed, so we can decide
whether to land it now or as a fast-follow.

## TL;DR

After #8004, **changing limits while cost tracking is enabled** works on reload. But **turning cost
tracking on** (`[cost] enabled = false → true`) — or, symmetrically, off→on toggling — still requires
a full process restart, because the process-global slot is a single-init `OnceLock` that permanently
caches `None` when the first boot had cost tracking disabled.

## Why the gap survives #8004

The global is:

```rust
static GLOBAL_COST_TRACKER: OnceLock<Option<Arc<CostTracker>>> = OnceLock::new();
```

`#8004` keeps the `OnceLock` and makes the *inner* `CostTracker` hold its config behind
`Arc<RwLock<CostConfig>>`, so `get_or_init_global` can `update_config(...)` the existing tracker on
every call. That works only when a tracker exists.

If the first caller on the current process runs while `config.cost.enabled == false`, the init
closure returns `None` and the `OnceLock` is now permanently `Some(None)` for the life of the
process. On a later reload with `enabled = true`:

```rust
let tracker = GLOBAL_COST_TRACKER.get_or_init(|| { /* not re-run; already initialized */ }).clone();
if let Some(ct) = tracker.as_ref() {   // None → skipped
    ct.update_config(config);
}
tracker                                  // returns None; no enforcement, no storage
```

The closure never re-runs (that's what `OnceLock` guarantees), so there is no tracker to update and
no storage to construct. Result: budgets silently stay off until the PID is recycled. This is the
same class of bug as the original incident (config frozen at first boot), just for the
`enabled`/storage-construction axis that lives in the `OnceLock` rather than in the config snapshot.

Note this is consistent with the reload model documented at
`crates/zeroclaw-runtime/src/daemon/mod.rs:14-18`: reload re-reads config and re-invokes
`daemon::run` **with the same PID**, so any process-lifetime static survives reload.

## Repro

1. Boot daemon with `[cost] enabled = false` (or no `[cost]` block).
2. Trigger one path that calls `CostTracker::get_or_init_global` (gateway/daemon/channels/agent loop
   all do at startup).
3. Edit config: `[cost] enabled = true`, set `daily_limit_usd` / `monthly_limit_usd`.
4. Reload (`/admin/reload` or whatever maps to it). 
5. **Observed:** no budget enforcement, no cost ledger writes; limits ignored.
   **Expected:** budget enforcement active with the configured limits.
6. Full stop/start fixes it (fresh `OnceLock`).

The inverse (on→off via reload) is milder but also wrong: `update_config` with `enabled=false`
makes `check_budget`/`record` early-return, so enforcement does stop — that direction is actually
fine. The broken direction is **off→on** (and any first-boot-disabled case), because storage was
never constructed.

## Suggested fix

Replace the single-init slot with a reloadable one so init can happen post-boot. Two options:

**Option A — `RwLock<Option<Arc<CostTracker>>>` (lazy construct/teardown):**

```rust
static GLOBAL_COST_TRACKER: OnceLock<RwLock<Option<Arc<CostTracker>>>> = OnceLock::new();

pub fn get_or_init_global(config: CostConfig, workspace_dir: &Path) -> Option<Arc<Self>> {
    let slot = GLOBAL_COST_TRACKER.get_or_init(|| RwLock::new(None));

    // Fast path: tracker exists → just hot-swap config (the #8004 behavior).
    if let Some(ct) = slot.read().as_ref() {
        ct.update_config(config);
        return Some(ct.clone());
    }
    // Disabled and none exists → nothing to do.
    if !config.enabled {
        return None;
    }
    // Enabled but no tracker yet (first boot was disabled): construct now.
    let mut guard = slot.write();
    if let Some(ct) = guard.as_ref() {          // re-check under write lock
        ct.update_config(config);
        return Some(ct.clone());
    }
    match Self::new(config, workspace_dir) {
        Ok(ct) => {
            let ct = Arc::new(ct);
            *guard = Some(ct.clone());
            Some(ct)
        }
        Err(e) => { /* same WARN record! as today */ None }
    }
}
```

- Keeps `update_config` and the `RwLock<CostConfig>` from #8004 — they remain the right mechanism
  for the common "change the limit" path.
- The outer `OnceLock<RwLock<…>>` is only there to give a stable, lazily-created lock; the
  `Option` inside is now mutable, which is the whole point.
- The double-checked `read()` then `write()` keeps the steady-state path lock-cheap and avoids
  constructing two trackers under a race.

**Decision needed:** do we also support on→off *teardown* (set slot back to `None` and drop storage
handles), or is it enough to leave the tracker resident and rely on `enabled=false` short-circuiting
inside it? I'd lean toward leaving it resident (simpler, and `update_config` already neutralizes
enforcement) and just documenting that storage handles persist until restart.

## Tests to add

- `get_or_init_global_constructs_tracker_when_enabled_after_disabled_boot`: first call with
  `enabled=false` → `None`; second call (reload) with `enabled=true` → `Some`, and `config()`
  reflects the new limits, and a subsequent call returns the **same** `Arc` (`Arc::ptr_eq`).
- Keep #8004's `get_or_init_global_applies_reloaded_config_to_existing_tracker` (enabled→enabled
  hot-swap, same `Arc`) — still valid.
- Optional: `enabled→disabled` reload leaves enforcement off (already true via `update_config`),
  asserting `check_budget` returns `Allowed`.

## Scope / risk

- Touches only `crates/zeroclaw-config/src/cost/tracker.rs` (the `get_or_init_global` body + the
  static type). No call-site signature changes — the 5 callers (gateway/lib.rs,
  runtime/daemon/mod.rs, runtime/agent/loop_.rs, channels/orchestrator/{mod,acp_server}.rs) keep
  calling `get_or_init_global(config.cost.clone(), &config.data_dir)`.
- risk: medium (same blast radius as #8004). Backward compatible; no config/CLI surface change.
- Could ship as a follow-up commit on `fix/cost-tracker-frozen-config-reload` or a separate PR —
  either way it should reference #8004 as the parent fix.
