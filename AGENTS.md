# Agent Instructions

## Commit messages

git-cliff (`cliff.toml`) auto-generates `CHANGELOG.md` and release notes from
the first line of each commit. Start it with a prefix:

- `fix` → Fixed
- `add`, `implement`, `support`, `introduce`, `check` → Added
- `rework`, `refactor`, `rename`, `remove`, `drop`, `make`, `scope`, `always`, `run`, `report` → Changed
- `docs`, `chore`, `ci`, `build`, `test`, `bump` → omitted

Use imperative mood, lowercase verb, no trailing period; details go in the
body. Regenerate with `mise run changelog` before tagging a release.

## Pull requests

Title PRs with the same naming pattern as commit messages (prefix + imperative
mood). Prefer squash merging so the PR title lands on `main` as a single,
changelog-friendly commit.
