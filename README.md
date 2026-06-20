# consign

Counts the unpushed work across a fleet of git repos, decides what's safe to push automatically, pushes it without ever force-pushing, and proves the debt reached zero.

## Why it exists

A fleet of repos accumulates push-debt quietly: a branch ahead of its remote, a repo with commits but no tracking branch, one with no remote at all. Eyeballing `git status` across dozens of directories misses exactly the cases that matter — the repo with no upstream reads as "clean" and gets dropped from the count. Consign classifies every repo into a named bucket so the undercount can't happen, then gates and drains the safe ones.

## Install

```sh
cargo install --path .
# or copy the release binary
cp target/release/consign ~/.local/bin/consign
```

## Quickstart

```sh
consign survey
```

Walks every immediate child of `~/wintermute` (or any `--root` you pass) and prints an aligned table of each repo's push-debt, with a totals footer. Add `--format json` for a machine-readable array; the output is pipe-safe (SIGPIPE handled, no panic).

```sh
consign survey --format json
consign survey --root ~/projects --root ~/wintermute   # multiple roots
consign survey | head -5
```

## The four verbs

Consign is a pipeline: enumerate, gate, push, prove.

| Verb | Does |
|---|---|
| `survey` | Classify every repo's push-debt into named buckets. |
| `policy` | Decide push-eligibility per repo: `auto-ok` \| `private-hold` \| `manual-only`. |
| `drain` | Push the `auto-ok` repos that are ahead or have no upstream. Dry-run by default. |
| `verify` | Re-survey and cross-check a drain receipt to confirm the debt is gone. |

### survey — push-debt classes

| Class | Meaning |
|---|---|
| `clean` | HEAD tracks a remote, nothing ahead |
| `ahead(n)` | Upstream set, n commits ahead of it |
| `no-upstream(n)` | Has a remote, no tracking branch; n commits exist on no remote |
| `no-remote` | No remote configured at all |
| `diverged(a/b)` | Upstream set, ahead a *and* behind b > 0 |

### policy — push-eligibility

- `auto-ok` — safe to push automatically: `ahead` / `no-upstream` / `no-remote` on the default branch, no hold marker, no detected secret.
- `private-hold` — never auto-pushed: name matches `*-private` or `autobuilder*`, a `.consign-hold` file is present, or a tracked secret-shaped file (`.env`, `*.pem`, `id_*`, `*credential*`) is detected.
- `manual-only` — needs a human: diverged, a non-default branch (worktree / feature), or detached HEAD.

Optional config at `~/.config/consign/policy.toml` (or `$CONSIGN_POLICY_CONFIG`) adds `extra_hold_globs` and `extra_hold_paths`. A missing config is not an error.

### drain — push the safe ones

```sh
consign drain                 # dry-run: print the push plan only (default)
consign drain --no-dry-run    # actually push
consign drain --only myrepo   # restrict to named repos (repeatable)
```

Drain runs survey + policy internally. For each `auto-ok` repo that is `ahead` or `no-upstream` it prints (dry-run) or executes (`--no-dry-run`) the push. Diverged and manual-only repos are never pushed — they appear in a "needs human" section. **Drain never uses `--force` or `--force-with-lease`.**

### verify — prove convergence

```sh
consign drain --no-dry-run --format json > receipt.json
consign verify --receipt receipt.json
```

Re-surveys the fleet and cross-checks the drain receipt. Verdicts: `converged` (no auto-ok debt remains, exit 0), `contradicted` (a repo the drain claimed it pushed is still ahead / no-upstream, exit 1), `residual` (auto-ok debt remains but no receipt supplied, exit 0 + warning). Manual-only / diverged / no-remote debt is always out of convergence scope.

## Scheduled drain (systemd-user)

`consign-drain.timer` runs `consign drain --no-dry-run` every 6 hours, matching the `adopt-cron` cadence. It appends a one-line summary to `~/brain/journal/YYYY-MM-DD.md` only when repos were pushed or errors occurred — a clean, zero-debt pass is silent.

```sh
bash contrib/install-cron.sh        # idempotent: copies units, reloads, enables, starts
systemctl --user list-timers | grep consign
systemctl --user disable --now consign-drain.timer
```

## Build and test

```sh
cargo build --release
cargo test
```

## License

MIT — Joe Yen <jyen.tech@gmail.com>
