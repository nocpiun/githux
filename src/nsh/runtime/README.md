# nsh-runtime: framework-independent account/session bridge

This module (`src/nsh/runtime/**`) is the plain-Rust bridge between the existing account/data
service layer and a Native Shell display. It owns no window, signal, focus or scroll handle; the NSH
adapter binds the returned snapshots to native signals. GPUI/`Entity`/`FocusHandle`/`ScrollHandle`
never appear in this module's interfaces.

## What it reuses

Every account, credential, cache and network operation goes through the existing services — no
second HTTP client, SQLite schema, credential target, cache path or business policy is introduced:

- `data::AccountService` — open/restore/verify/switch/replace/remove/revalidate, cache ledger.
- `data::RepoService` — `list_repos_cached` (online + historical fallback) and page sessions.
- `storage::cache::CacheStore` — the single application cache, opened exactly once (see `backend`).
- `network::executor::BackgroundExecutor` — the existing Tokio background runtime; all storage and
  network work runs there, never on the display/UI thread.

The bridge is **not** a no-op: commands drive real mutations, the sidebar read composes the real
snapshot adapter, page controllers receive real services, and cancellation/revocation have real
gates. It only deliberately does not render anything.

## Public surface

| Type | Role |
| --- | --- |
| `RuntimeHandle` | one per application; the display calls `drain`/`snapshot` and `submit` |
| `Snapshot` | owned presentation state the adapter binds to signals on every drain |
| `Command` | account mutations + `RetryRepositories`; PATs move in and never appear in snapshots |
| `Completion` | per-command result, including `CancelledSettled` (no rollback claim) |
| `Failure` | typed error: login / account / GitHub / cache / not-cached / worker |
| `ExitGate` | process-exit barrier for background teardown |
| `Backend` | service/cache/executor ownership and idempotent nonblocking `shutdown` |
| `Services` / `PageContext` | real service handles a page binds to; invalidated on account-mutation intent |
| `PageTicket` / `PageSlot<T>` | per-section request identity and gated publication for a value |
| `Sessions` / `Workspace` / `Route` / `TabId` / `PageIdentity` | account-scoped navigation memory |

## The drain contract (read before wiring)

The NSH adapter must run the same obligations the old GPUI controller did, in native form:

1. **Drain on events and on a light foreground tick.** `RuntimeHandle::snapshot()` (or `drain()`)
   synchronizes authoritative account state and applies queued completions. A `wake` callback
   passed to `RuntimeHandle::start` should schedule the next drain from the background thread; it
   must not touch UI signals directly.
2. **Bind the whole `Snapshot` on each drain and remove/replace old signal values.** A snapshot is
   authoritative: when `snapshot.accounts` becomes empty, the previous account rows must be cleared
   by the binding step, not kept because this drain "didn't mention them". This is what makes
   removal, sign-out and revocation clear the screen.
3. **Check revocation even when the inbox is empty.** `drain()` re-checks the displayed
   authorization token with atomic loads only (no SQLite/network), so a repository revocation or a
   cache-gate invalidation clears the displayed list in a single drain without a timer probe
   (LESSONS L022).
4. **Never treat a failure as a valid empty value.** `RepositoryList::Failed(_)` is not rows;
   `PageSlot::value` returns `None` for invalid, not a synthesized empty value; `Services::None` and
   `PageContext::cache() == None` mean "no binding", never "empty content".
5. **Honour the exit gate.** Clone `ExitGate` before the window opens; wait on it only **after**
   the native event loop returns. `wait` returns `false` on timeout, never claiming durable work
   finished. Do not `block_on` it on the UI thread.

## Commands and cancellation

- `submit(Command)` rejects with `SubmitError` while another command is in flight, before the
  backend is ready, while closing, or for an empty PAT. The result is one `Completion` per command.
- `cancel()` cancels the in-flight operation. A repository read cancels and exposes a recoverable
  `Failed(Cancelled)`. An account mutation is **not rolled back**: the reply is suppressed and the
  submit gate stays busy until the service settles, then the snapshot reconciles authoritative
  state. `Completion::CancelledSettled` says exactly that.
- The token moves out of the UI into the command and is rebuilt never into a snapshot or callback;
  `Pat` has no `Debug`/`Display`/`Serialize`.

## Page binding

```rust
let services = handle.services()?;               // None while logged out / mid-mutation
let tab = handle.sessions_mut().open(Route::Repository { .. });
let ctx = services.page_context(handle.sessions(), tab, Some(repo_id))?;
let mut slot: PageSlot<T> = PageSlot::default();
let ticket = slot.begin(ctx);
services.spawn(async move { /* real read on the existing executor */ }).await?;
slot.publish(ticket, value, handle.sessions());  // gated on account+incarnation+tab+resource+auth
// each drain:
if slot.clear_invalid(handle.sessions()) { /* drop native state bound to the old value */ }
if let Some(value) = slot.value(handle.sessions()) { /* bind */ }
```

`PageContext` exposes `repository_session()` / `page_session()` so a page reaches the real
repository and page-data services without constructing its own account or cache. `is_current` folds
the binding-revision gate, the workspace identity (account + incarnation + tab + resource) and the
captured cache authorization; `Services::invalidate` flips the binding at mutation **intent**, so a
page can never keep publishing into the next account even before the commit is observed.

## Wiring points for the shell/base author

1. Declare the module in the owned tree (`src/nsh/mod.rs`: `pub mod runtime;` and `mod nsh;` in
   `main.rs`). Cargo/module wiring and the NSH dependency remain base-author-owned; no new crate
   dependency is required by this module.
2. Create the handle on the UI thread: `RuntimeHandle::start(ExitGate::default(), Some(wake))`, or
   `RuntimeHandle::from_backend(backend)` when adopting an already-assembled service.
3. Bind `Snapshot` fields to NSH signals (`signal(..)` / `set`) after every `snapshot()`, and bind
   the active workspace to `Sessions` so tabs restore per account.
4. Run `ExitGate::wait` after the event loop returns.
5. The NSH Reader/image/theme tasks and every page component consume `PageContext`/`PageSlot`; this
   module does not render them and does not decide the final Settings layout.

## Verification

The bridge is verified offline against real services, real SQLite/CacheStore, in-memory credentials
and a mock transport (no GitHub, no real PAT): account A/B tab isolation and case-alias routing,
late-result rejection across switch and A→B→A, PAT replace/remove invalidation, cache-open fallback
to online reads, revocation clearing a displayed page in one drain, and the exit gate's
drain/timeout behaviour. See `.agents/task/implementation-result.md` and the task runs under
`.agents/task/runs/`.

Because `Cargo.toml`/`src/nsh/mod.rs` are owned by the base author, the isolated probe manifest at
`.agents/task/probe/core/Cargo.toml` compiles these sources plus the needed service layer and runs
the tests: `node scripts/verify.mjs -- cargo test --manifest-path .agents/task/probe/core/Cargo.toml --lib`.
