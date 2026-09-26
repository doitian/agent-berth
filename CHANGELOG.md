# Changelog

All notable changes to this project are documented in this file.

## [3.0.1](https://github.com/doitian/agent-berth/releases/tag/v3.0.1) - 2026-09-26

### Changed

- Rename list age column to ago ([0df869d](https://github.com/doitian/agent-berth/commit/0df869d6012e66873a22b43c39d39d58a06fed33))

### Fixed

- Fix: stop live transcript preview flashing loading on refresh ([1cc8b0d](https://github.com/doitian/agent-berth/commit/1cc8b0dce8debd72b94abd60533e8b7cd59167ad))
## [3.0.0](https://github.com/doitian/agent-berth/releases/tag/v3.0.0) - 2026-09-26

### Added

- Check: report outdated agent integration config in doctor ([c3aa54d](https://github.com/doitian/agent-berth/commit/c3aa54d01d2ccefa9d3251bebdcb0a3e14dd551c))
- Add project icon for agent-berth ([2146164](https://github.com/doitian/agent-berth/commit/2146164ffcee5b5a3623601ee2061f3fbbcda3d3))
- Add opencode 2 plugin package with version-aware install ([b9c6543](https://github.com/doitian/agent-berth/commit/b9c6543f5325d4c87dac42b5e810bdf7a8c49e7b))
- Support opencode 2 session tabs in pane resolution and preview ([bd4e7b9](https://github.com/doitian/agent-berth/commit/bd4e7b981893d63e0b2fb3544c682e5449829c8d))
- Support conversation preview for cli sessions without a front pane ([a621613](https://github.com/doitian/agent-berth/commit/a621613ff7d8fc3e6ae45db15f988bc51ab011f5))
- Support pi transcripts in conversation previews ([f41c007](https://github.com/doitian/agent-berth/commit/f41c0074ff4352731a2e009796b25a1482a24137))

### Fixed

- Fix: keep finished claude sessions from showing as running ([136f4c1](https://github.com/doitian/agent-berth/commit/136f4c195b2128e25f7c8f1f0738ac46fd9b3c9d))
- Fix: ignore stale agent rows when ending a claude run ([92b5d1f](https://github.com/doitian/agent-berth/commit/92b5d1f303d9ed5dbde687e96d32252cd1b31ee0))
- Fix: switch clients when a tmux binding starts the TUI ([c8457ad](https://github.com/doitian/agent-berth/commit/c8457add225078637cc79caa6b80337de4bf34a3))
- Fix: resolve windows shims when detecting the opencode version ([29cc33d](https://github.com/doitian/agent-berth/commit/29cc33d9cc7bb61922cdbc901e0b5c0e23f220e5))

### ⚠️ Breaking Changes

- Fix: run claude turn-terminal hooks synchronously ([44d8cc2](https://github.com/doitian/agent-berth/commit/44d8cc2aa291364a9f11d83f2bebc702eb0b725e))

## [2.2.1](https://github.com/doitian/agent-berth/releases/tag/v2.2.1) - 2026-09-22

### Changed

- Rework: use brand-aligned TUI logos for grok and opencode ([17f2d83](https://github.com/doitian/agent-berth/commit/17f2d83cad3ff7a2279348fb58282fe511a6b2a3))

## [2.2.0](https://github.com/doitian/agent-berth/releases/tag/v2.2.0) - 2026-09-22

### Added

- Add worktree-aware labels to directory sort ([f615ccc](https://github.com/doitian/agent-berth/commit/f615ccc0a7d226d203678fd6653a49bffd15fe03))
- Add tui --tmux to attach or spawn the TUI in a tmux window ([6c6dc3e](https://github.com/doitian/agent-berth/commit/6c6dc3e804ed592dfe1794fab61a88905ad8b76c))
- Add stats subcommand aggregating active sessions by status and provider (#10) ([a171d08](https://github.com/doitian/agent-berth/commit/a171d08f3e877becd8e16b108940687e8cfc6f36))

### Changed

- Rework: serve list from heartbeat-maintained store ([1b04454](https://github.com/doitian/agent-berth/commit/1b0445403a927c858e477322edcb0d45ca6ad69f))

### Fixed

- Fix integration test binary path with symlinked target dir ([70a32e2](https://github.com/doitian/agent-berth/commit/70a32e2e959f8d3026af9719d811df589f62e0b1))

## [2.1.0](https://github.com/doitian/agent-berth/releases/tag/v2.1.0) - 2026-09-22

### Added

- Add Claude desktop session focus from the TUI ([56a75bb](https://github.com/doitian/agent-berth/commit/56a75bb10a498e5b3040977f3c653912d375adb1))
- Add codex desktop session focus from the tui ([ac3f209](https://github.com/doitian/agent-berth/commit/ac3f20987e752a135734c0cc04e910912b45c727))

### Fixed

- Fix codex cli and desktop source detection ([2400888](https://github.com/doitian/agent-berth/commit/24008884f1ddd6cb15a30e1736074caf13895739))

## [2.0.0](https://github.com/doitian/agent-berth/releases/tag/v2.0.0) - 2026-09-22

### Added

- Add desktop conversation previews for Claude and Codex (#8) ([7cc1aa7](https://github.com/doitian/agent-berth/commit/7cc1aa7247f57a4cc8ba67ae887bc1e185a0d60b))

### Fixed

- Fix duplicate Claude desktop sessions during discovery (#7) ([9c001db](https://github.com/doitian/agent-berth/commit/9c001db98d78fb3547cea26a89026c1c35caeb6b))

### ⚠️ Breaking Changes

- Add running status and ordered status sort (#9) ([4e87daf](https://github.com/doitian/agent-berth/commit/4e87dafed19d543d61c6968754346c8f18bddeb4))

## [1.2.1](https://github.com/doitian/agent-berth/releases/tag/v1.2.1) - 2026-09-21

### Fixed

- Fix attach preview on windows ([b6f2bb8](https://github.com/doitian/agent-berth/commit/b6f2bb8b78d5c8445bab3b40f35b6ae389cac5c8))

## [1.2.0](https://github.com/doitian/agent-berth/releases/tag/v1.2.0) - 2026-09-21

### Added

- Add catppuccin latte styling and git status symbols ([5e12575](https://github.com/doitian/agent-berth/commit/5e125751319361f073277f42be233f031d04a2ce))
- Add toggleable group headers to the tui list ([ad960f4](https://github.com/doitian/agent-berth/commit/ad960f4c1b98728a6a7658f3a70078c0f0e1952c))

### Changed

- Rework tui list rows with padded status initials ([7b798b2](https://github.com/doitian/agent-berth/commit/7b798b284d857c5b559b2adffacf6d433a08cee4))

### Fixed

- Fix codex waiting status during automatic approval review ([7be7940](https://github.com/doitian/agent-berth/commit/7be794015202b1ed2e58f0c7ea2d49795dcbb6d3))
- Fix stale codex waiting state during automatic approval review ([5671fba](https://github.com/doitian/agent-berth/commit/5671fba646f9da2886498cff931dabfccccc1241))
- Fix tmux preview losing terminal colors ([98efd3c](https://github.com/doitian/agent-berth/commit/98efd3c242ab71e217cc788e0eee86e716b238b5))

## [1.1.0](https://github.com/doitian/agent-berth/releases/tag/v1.1.0) - 2026-09-21

### Added

- Add interactive TUI mode (#1) ([cebd277](https://github.com/doitian/agent-berth/commit/cebd2778b146a87caa664e6ac25e278411128d37))
- Add: rewrite pi skill block titles as slash commands ([58d9d39](https://github.com/doitian/agent-berth/commit/58d9d3972e35161aa3566e015a256d8f5a654437))
- Add git status and GitHub shortcuts to the tui ([272c9dc](https://github.com/doitian/agent-berth/commit/272c9dcc6728fc1b8e478cb76130805e3c34920c))
- Add d shortcut to delete a session from the tui ([80707e2](https://github.com/doitian/agent-berth/commit/80707e2fd97573c655a814b003b83267b6247a3b))
- Add session sort shortcuts to the tui ([5ff2ea5](https://github.com/doitian/agent-berth/commit/5ff2ea5b02a4d6ad53975fdc80ca9ca72e9cd3a1))

### Changed

- Always scroll attach preview to the pane bottom ([74bd172](https://github.com/doitian/agent-berth/commit/74bd1724aca5bbe72424e5249022beb63ad550c6))
- Make the TUI session list order stable (#3) ([1b5830a](https://github.com/doitian/agent-berth/commit/1b5830a4ea0873f0dd718b22175859badb70bfcc))
- Rework git branch and status display in tui details (#5) ([71cc06d](https://github.com/doitian/agent-berth/commit/71cc06d9d11f7acf7b46f6392f5d160bb1f101a6))

### Fixed

- Fix: wait for server shutdown on Windows service stop ([21bd27c](https://github.com/doitian/agent-berth/commit/21bd27cd833d04a26ee13c7237882434b47d82bb))
- Fix blocking session navigation and cache pane previews ([b67014c](https://github.com/doitian/agent-berth/commit/b67014c274ec441c46d4af80c6601bb6832a5e2b))
- Fix missing codex session titles ([99841d7](https://github.com/doitian/agent-berth/commit/99841d795185825a502ee7b13134f8b576ab567a))

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
