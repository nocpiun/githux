# Devlog: verification-workflow

Date: 2026-10-31
Task: verification-workflow
Branch: chore/verification-workflow
Base: 536e3af12c63c6244aaa0b7d69c15598b4f37176

## Goal

Implement approved workflow optimizations 1, 2 and 3:
1. A reliable shared local verification command
2. A final-verification order that avoids avoidable reruns
3. Concise task prompts emphasizing product invariants and real input evidence

Deliver through a PR to main with milestone #1.

## What was implemented

### 1. Shared verification runner (`scripts/verify.mjs`)

- Single documented invocation: `node scripts/verify.mjs [options] -- <executable> [args...]`
- Arguments passed as array after `--`; no shell text interpolation
- Preserves actual child exit code; full stdout/stderr captured to unique log files
- Atomic per-run JSON record under `.agents/task/runs/<run_id>.json` with:
  - run_id, command, args, worktree
  - source_identity (head SHA, dirty status, changed file count, status sample)
  - started_at, finished_at, status, waiting_for_slot_ms, running_duration_ms, wall_duration_ms
  - exit_code, signal, spawn_error, timed_out, child_pid, log_path
- Failed/interrupted/unfinished records are never reported as green
- Windows shared build slot via `Local\Githux-Cargo-20261029` mutex (held by a PowerShell helper process)
- Portable execution path: non-Windows platforms skip the mutex
- Start failure, cancellation, and timeout are handled honestly:
  - Timeout kills child and marks record as `timed_out` (never success)
  - Spawn errors are captured in `spawn_error`
  - Resources released only after child is known stopped
- Concise output: run ID, outcome, exit code, command, durations, log path
- `--json` flag for full record output; `--timeout <ms>`, `--no-lock`, `--task-dir <dir>` options

### 2. Final-verification order and task prompt improvements

- Updated `docs/agent-workflow.md`:
  - Verification command convention section
  - Local verification order: baseline + real input contract → acceptance cases → implementation/focused regressions → preview/docs → final complete verification → commit/push/review
  - After final checks pass, rerun only for actual change, failure, or unresolved concern
- Updated dispatch handoff template (`dispatch.mjs`):
  - Added `## Verification runner` section to every generated TASK.md
  - Documents the exact invocation, argument array usage, record location, mutex, and rerun policy
- Updated `githux-agent-dispatch/SKILL.md`:
  - Task specs should be concise: goal, hard constraints, real counterexamples, ownership/dependencies, acceptance evidence, completion conditions
  - External API assumptions need current primary docs and representative real response shapes
  - Tests stay deterministic/offline

### 3. CI integration

- Added `node --test scripts/verify.test.mjs` to `script-check` job in `.github/workflows/ci.yml`
- Updated `ci-scope.mjs` to classify `scripts/verify.mjs` and `scripts/verify.test.mjs` as script-only changes (skip Rust build/check)
- Updated `ci-scope.test.mjs` with coverage for the new classification
- Updated `docs/code-standards.md` to list the new test command and describe coverage

## Tests

11 offline tests in `scripts/verify.test.mjs`:

1. `computeSourceIdentity` captures head SHA and dirty state
2. `parseRunArgs` parses flags and command correctly
3. `executeVerifiedCommand` captures stdout, stderr, and exit code (exit 42)
4. `executeVerifiedCommand` handles successful command
5. `executeVerifiedCommand` handles paths and args with spaces and Unicode
6. `executeVerifiedCommand` handles missing executable (ENOENT)
7. `executeVerifiedCommand` handles timeout and preserves record (never green)
8. `executeVerifiedCommand` serializes runs with Windows mutex (harmless fixture children)
9. CLI preserves child exit code (exit 9)
10. Real Cargo invocation: failed `cargo check --lib --nonexistent-flag` preserves non-zero exit
11. Real Cargo invocation: `cargo --version` succeeds with truthful record

All tests pass. Existing dispatch tests (8), CI scope tests (6) continue to pass.

## Limitations

- No real heavy Rust build was run through the runner (only `cargo --version` and a failing `cargo check` with invalid flags)
- Mutex serialization tested with short-lived Node.js children, not actual concurrent Cargo builds
- Windows-only mutex; CI on Linux/macOS runs without the lock (as designed)
- The runner does not auto-skip required checks or cache green results

## Files changed

- `scripts/verify.mjs` (new, 562 lines)
- `scripts/verify.test.mjs` (new, 276 lines)
- `.agents/skills/githux-agent-dispatch/SKILL.md` (+1 line)
- `.agents/skills/githux-agent-dispatch/scripts/dispatch.mjs` (+handoff verification section)
- `.github/scripts/ci-scope.mjs` (+2 lines)
- `.github/scripts/ci-scope.test.mjs` (+2 lines)
- `.github/workflows/ci.yml` (+3 lines)
- `README.md` (+runner documentation)
- `docs/agent-workflow.md` (+verification order and command convention)
- `docs/code-standards.md` (+test command and description)
