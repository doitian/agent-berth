# Changelog

All notable changes to this project are documented in this file.

## [1.0.1](https://github.com/doitian/agent-berth/releases/tag/v1.0.1) - 2026-09-20

### Added

- Add README ([7ab7d42](https://github.com/doitian/agent-berth/commit/7ab7d42fc3f4dce796b0ab7bad0aeae641840516))
- Add auto-generated changelog with git-cliff ([7df3af6](https://github.com/doitian/agent-berth/commit/7df3af66a7d914ed44d13c49538bef2ff2b7edb1))

### Changed

- Scope typos ignore to commit SHA patterns ([5ecfcff](https://github.com/doitian/agent-berth/commit/5ecfcffdea0b9247ad0d0949f6841b1334569699))

### Other

- Use mbx action and cache in CI tests ([5f1c3db](https://github.com/doitian/agent-berth/commit/5f1c3db6aadd8b3b44230ced39fb5f41f976d4ef))

## [1.0.0](https://github.com/doitian/agent-berth/releases/tag/v1.0.0) - 2026-09-20

### Added

- Add agent-bridge CLI to monitor and resume coding agents ([deea811](https://github.com/doitian/agent-berth/commit/deea8116724fc405eefd37b29050286d94f4a59d))
- Add agent-berth integration tests and release automation ([318c54b](https://github.com/doitian/agent-berth/commit/318c54b2b89f07e4e8a1543056a4a599b810cd92))
- Add mock llm client tests ([8a502f9](https://github.com/doitian/agent-berth/commit/8a502f951210fd017fe70de12cd134cf06630754))
- Add attach to running agents in tmux ([ee548ea](https://github.com/doitian/agent-berth/commit/ee548eaee419016776982c539c464cdb29465c3d))
- Check fzf in doctor ([e7e45ed](https://github.com/doitian/agent-berth/commit/e7e45ed5ad97f78e663a1f593337a98e02b02e15))
- Add clippy to CI and fix lints ([e2a253f](https://github.com/doitian/agent-berth/commit/e2a253fc7697031660b3a79e0bdb67e001f10e51))

### Changed

- Drop stale desktop sessions and cap Codex hook timeouts ([a20b544](https://github.com/doitian/agent-berth/commit/a20b544f9801e7f584f28e4a64a0b25afac611c4))
- Rename bridge terminology to berth ([eb48383](https://github.com/doitian/agent-berth/commit/eb483830a6ce0d09e0a934c07d652801a772221a))
- Run mock llm client tests in ci matrix ([8320ba2](https://github.com/doitian/agent-berth/commit/8320ba2a106fb2982fe708704719bce6b5e5586d))
- Remove release task and path env from mise.toml ([77dea21](https://github.com/doitian/agent-berth/commit/77dea2194e812a6f7570dae8c8291a354227d989))
- Report session titles for pi and opencode ([8d01afb](https://github.com/doitian/agent-berth/commit/8d01afb5fbc39358f7217346cc2a96bd14c49418))
- Report pi session titles before the first turn settles ([b288cf7](https://github.com/doitian/agent-berth/commit/b288cf7debead2f3a53b8b7772c14523299b34fe))
- Make attach pane preview opt-in with -p ([bf77860](https://github.com/doitian/agent-berth/commit/bf778607d26f04c47723127f9d0422551af81987))
- Scope attach to the current tmux session with -s ([5eb57e5](https://github.com/doitian/agent-berth/commit/5eb57e5a8823c73b930048891c5bf121d55bb1b1))
- Always show fzf when attaching ([0965f4a](https://github.com/doitian/agent-berth/commit/0965f4a8ef92be3a6edd5dde95c8cdfc64719f09))
- Rework resume with idle opt-in, selection, rm, and pruning ([a4afa00](https://github.com/doitian/agent-berth/commit/a4afa0088ed6a35c470cea11cf7db71ed607319e))

### Fixed

- Fix runner context usage in test workflow ([5c4063e](https://github.com/doitian/agent-berth/commit/5c4063ed0a71c84050668d427e7c2529efece0f2))
- Fix hooks for real clients on windows and codex exec ([bb45b01](https://github.com/doitian/agent-berth/commit/bb45b01ec4319bd5e93a81ccd6ebec982c59bf99))
- Fix mise install task on windows ([9fe9562](https://github.com/doitian/agent-berth/commit/9fe95625e37492edca608c3f6311ec85acbd3abb))
- Fix windows service setup and add service subcommands ([1bb755d](https://github.com/doitian/agent-berth/commit/1bb755dbe503858d0df3220acf314197a652979d))
- Fix attach pane parsing and Windows mock client lookup in CI ([1025470](https://github.com/doitian/agent-berth/commit/10254707d736c397e6b33b3391ff677e6ce46551))
