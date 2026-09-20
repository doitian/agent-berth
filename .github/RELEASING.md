# Releases

Push a tag named exactly `v` followed by the package version in `Cargo.toml`, for
example `v0.1.0`. All tag pushes trigger `publish.yml`; tags with another name or
version fail before tests, binary builds, authentication, or publishing.

Before tagging, regenerate `CHANGELOG.md` with `mise run changelog` (requires
[git-cliff](https://git-cliff.org)) and commit it. Commits are grouped by
message prefix (`fix` becomes Fixed, `add` becomes Added, and so on; see
`cliff.toml`). The release workflow extracts the tag's changelog section and
uses it as the GitHub Release notes.

The workflow validates the crate package, runs the reusable Windows/Linux test
suite, and builds these binaries:

| Target | Archive |
| --- | --- |
| `x86_64-pc-windows-msvc` | `.zip` |
| `x86_64-unknown-linux-gnu` | `.tgz` |
| `x86_64-apple-darwin` | `.tgz` |
| `aarch64-apple-darwin` | `.tgz` |

Each archive is named `agent-berth-<target>-v<version>.<format>` and contains
`agent-berth` (or `agent-berth.exe`) at its root. Each binary is checked with
`--version` before packaging. SHA-256 checksum files accompany the archives.
Archives also include the MPL-2.0 license and a link to the tagged source code.
Linux binaries are built on Ubuntu 22.04 and require a compatible glibc.

After all checks and builds pass, the workflow creates a GitHub Release with the
archives, then publishes the crate to crates.io. This order makes binaries
available before a new registry version can be installed with:

```powershell
cargo binstall agent-berth
```

`Cargo.toml` contains the exact download URL and archive-layout templates. Its
`repository` points to `https://github.com/doitian/agent-berth`. Targets without a matching
archive retain cargo-binstall's normal fallback behavior. See the
[cargo-binstall metadata reference](https://github.com/cargo-bins/cargo-binstall/blob/main/SUPPORT.md).

## Trusted publisher setup

Before the first automated release:

The package uses MPL-2.0, and the `crates-io` GitHub environment is configured.

1. In the crate's crates.io settings, register a GitHub trusted publisher using
   owner `doitian`, repository `agent-berth`, workflow filename `publish.yml`, and
   environment `crates-io`.
2. Ensure the publishing account owns `agent-berth` and has completed
   crates.io's account requirements. New crates require an initial token-based
   publication before a trusted publisher can be registered; subsequent versions
   use the tag workflow. See the
   [Rust project's trusted publishing announcement](https://blog.rust-lang.org/2025/07/11/crates-io-development-update-2025-07/).

The publish job has `id-token: write` and uses
[`rust-lang/crates-io-auth-action`](https://github.com/rust-lang/crates-io-auth-action)
to exchange GitHub OIDC identity for a short-lived token. No long-lived
`CARGO_REGISTRY_TOKEN` secret is required. The token is exposed only to the
`cargo publish --locked` step and revoked by the action afterward. See the
[crates.io trusted publishing guide](https://crates.io/docs/trusted-publishing).

If registry publishing fails after the GitHub Release succeeds, fix the registry
configuration and rerun only the failed jobs. Recreating an existing GitHub
Release or republishing an existing crate version is intentionally not automatic.
