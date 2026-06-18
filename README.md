# consign

Accurate fleet push-debt enumerator for git repos.

## TL;DR

`consign survey` walks every immediate child of `~/wintermute` (or any `--root` you specify) and classifies each git repo's push-debt into named buckets: `clean`, `ahead(n)`, `no-upstream(n)`, `no-remote`, `diverged(a/b)`. It fixes the systematic undercount in self-review that silently dropped repos with no upstream tracking branch.

## Install

```sh
cargo install --path .
# or copy the release binary
cp target/release/consign ~/.local/bin/consign
```

## Usage

```sh
# Human-readable table (default)
consign survey

# Machine-readable JSON
consign survey --format json

# Override root directory (may be passed multiple times)
consign survey --root ~/projects --root ~/wintermute

# Safe to pipe — SIGPIPE handled, no panic
consign survey | head -5
```

## Output classes

| Class | Meaning |
|---|---|
| `clean` | HEAD is on a remote, nothing ahead |
| `ahead` | Upstream set, n commits ahead of it |
| `no-upstream` | Has a remote, but no tracking branch; n commits exist on no remote |
| `no-remote` | No remote configured at all |
| `diverged` | Upstream set, ahead a AND behind b > 0 |

## Acceptance criteria

1. `--format json` returns a JSON array with `path`, `branch`, `class`, and class counts per repo.
2. Repos with a remote but no tracking branch are classified `no-upstream` with non-zero count (not `clean`).
3. `no-remote`, `ahead(n)`, and `diverged(a/b)` are correctly classified with exact counts.
4. `--format table` prints an aligned table with a totals footer summing all classes.
5. `--root <dir>` overrides default, multiple roots supported, non-git dirs silently skipped, unreadable root returns structured error.
6. `consign survey | head -1` does not panic (SIGPIPE reset).
7. `cargo test` green; binary produced at `target/release/consign`; `--help` lists `survey`.

## License

MIT — Joe Yen <jyen.tech@gmail.com>
