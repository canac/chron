# Known races

Two races found in the branch since `origin/main` that were not fixed, with the commit
that introduced each in case the change should be reverted instead of repaired.

Commit hashes are current as of writing. Any rebase rewrites them, so the commit
*subjects* are the stable identifier.

---

## 1. `handle.abort()` can cancel a run's database completion

- **Introduced by:** `7510430` — *Stop retrying terminated runs*
- **Location:** `src/chron_service/mod.rs:384`
- **Severity:** medium

The scheduler's overrun branch runs `job.terminate().await` and then unconditionally
`handle.abort()`:

```rust
() = sleep_until(next_run) => {
    warn!("{name}: terminating existing run");
    job.terminate().await;
    handle.abort();
}
```

If the child exits at almost the same instant `sleep_until(next_run)` fires,
`exec_command_once`'s own `select!` has already taken the `process.wait()` branch and
dropped `rx_terminate`. `tx_terminate.send(())` therefore fails, `Process::terminate`
short-circuits to `false` **without awaiting `rx_terminated`**, and `Job::terminate`
returns `None`. `abort()` then cancels the exec task while it is awaiting
`db.complete_run(...)`.

Consequences:

- The run row stays at `state = 'running'` with a pid, until the next `Database::init`
  sweeps it to `'completed'` on host start.
- The job's `next_run` is never written.
- `chron status` and the web UI show the job as running, while `chron kill` reports
  `NotRunning` — the in-memory `running_process` was already taken by `terminate()`.

**Why it was left alone.** Since the same commit makes `terminated` yield
`next_attempt == None`, the exec task exits on its own, so the `abort()` buys very little.
The obvious repair — `let _ = handle.await` instead of `abort()` — reintroduces a hang: if
`terminate()` found no process because the job is sleeping between retry attempts, the
scheduler loop blocks until that retry finishes. A correct fix needs either a bounded
grace period before aborting, or moving `db.complete_run` somewhere cancellation cannot
reach it.

**To revert:** delete the single added `handle.abort();` line. Keep the rest of the commit
— the `if terminated { None }` change in `exec.rs` is what makes the abort unnecessary.

---

## 2. Same abort race on `trigger_handle`

- **Introduced by:** `ce1d8bf` — *Preserve original run handle when triggering*
- **Location:** `src/chron_service/mod.rs:243`
- **Severity:** low

```rust
if let Some(trigger_handle) = task.trigger_handle {
    trigger_handle.abort();
    let _ = trigger_handle.await;
}
```

Identical window to race 1. If a triggered run's process has already exited and the task
is mid-`db.complete_run`, `job.terminate()` returns `None` and the abort cancels the
write, stranding the run at `state = 'running'` across a chronfile reload.

**To revert:** reverting *Preserve original run handle when triggering* wholesale is not advisable — it restores
`task.handle = spawn(...)` in `trigger()`, which clobbers the scheduler's own join handle
(the bug that commit exists to fix). Remove only the `trigger_handle.abort()` /
`await` block, or fix it together with race 1.

**Note:** the pre-existing `task.handle.abort()` two lines above
(`src/chron_service/mod.rs:235`, predates `origin/main`) has the same hazard. Reverting
either commit above does not cure it; the abort-then-await pattern needs to be fixed once
and applied to all three call sites.
