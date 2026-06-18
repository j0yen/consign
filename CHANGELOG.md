# Changelog

## v0.4.0 — 2026-06-18

consign publish: mint j0yen/<name> remote for no-remote repos; dry-run default; honest abort on auth/collision; per-repo receipts

## v0.3.0 — 2026-06-18

consign drain: push eligible repos safely — ahead gets git push, no-upstream gets push --set-upstream; dry-run default; diverged/manual skipped; serialized with per-repo receipts

## v0.2.0 — 2026-06-18

consign policy: default-deny push-eligibility gate; auto-ok|private-hold|manual-only; classify() exported for drain/publish; secret detection + private-repo allowlist
