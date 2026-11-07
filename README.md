<p align="center">
  <img src="./poly.svg" alt="Poly" width="320">
</p>

# Poly

**One build system for Rust, C/C++ and Go.**

Poly is a build system and package manager for Rust, C/C++ and Go by [Axiom](https://github.com/axiomwtf). It uses one manifest (`poly.ini`), one lock file (`poly.lock`), and one command set, and invokes `rustc`, C/C++ compilers, and the Go toolchain directly instead of Cargo, CMake, Make or go mod for user projects.

> **Status:** `0.2.1` - available.

## Why

- **Native, not a wrapper.** Poly calls `rustc`, `gcc`/`clang`/`msvc` and `go tool` directly, then does its own dependency resolution, build graph, incremental compilation and linking.
- **One manifest.** A single `poly.ini` covers targets, dependencies, profiles, publishing, formatting, toolchains, cross-compilation, plugins and hooks.
- **Signed packages.** `poly build` produces `.vfsx.tar.zst` packages that are verified with the external `vfsx` tool before extraction.
- **Cross-compiles by default.** Every `poly build` produces Windows and Linux binaries; there is no flag to pick one OS.
- **Polyglot packages.** One package holds one executable per platform, built from every language in that package.

## Example

```ini
[project]
name = my-app
version = 1.0.0
edition = 2024

[target.app]
language = rust
type = bin
sources = src/main.rs

[dependencies.rust]
serde = 1.0|derive
tokio = 1.40|full
```

```bash
poly init --name my-app
poly add rust:serde
poly build
poly build --release
```

## Usage

```bash
# Core
poly init
poly build [--release] [--no-package]
poly test [target]
poly clean

# Dependencies
poly add <lang>:<dep>
poly remove <lang>:<dep>
poly update [<lang>:<dep>]
poly outdated
poly tree
poly lock

# Packages
poly package
poly sign
poly verify
poly install <package>
poly uninstall <package>

# Quality
poly lint [target]
poly fmt
poly doc
poly ci <provider>
poly migrate cargo|go-mod|cmake
```

`poly build` writes `dist/{profile}/` and finishes with one combined `{name}-{version}-{profile}.vfsx.tar.zst` package at the `dist/` root; `--no-package` keeps the build tree for inspection.

## Configuration

`poly.ini` uses dotted sections for hierarchy, commas for arrays, and `|` for dependency metadata such as `serde = 1.0|derive`. Keys are camelCase (`crateName`, `tokenEnv`, `includeDocs`).

```ini
[publish]
crateName = my-org-my-app
targets = rust:app
platforms = x86_64-linux, x86_64-windows
tokenEnv = MY_REGISTRY_TOKEN

[hooks]
preBuild = "echo building"
postBuild = "echo done"
```

## Packages

```bash
poly package
poly sign
poly verify
poly install my-app
poly uninstall my-app
```

`poly publish` uploads Rust targets to crates.io as source packages containing `poly.ini`, `poly.lock` and a stub `Cargo.toml`. `poly install` downloads sources, builds them with Poly and installs the result. `poly login`/`poly logout` store registry tokens in `~/.config/poly/credentials.toml`.

## Limitations

- The resolver is greedy latest-match, not a full SAT solver.
- C/C++ and Go drivers work but are less battle-tested than Rust.
- Git dependencies, `[patch]`/`[replace]`, and dev-dependencies are not yet supported.

## Building from source

Poly is written in Rust (edition 2024). The CLI lives in [`crates/poly`](./crates/poly) and the supporting crates live in [`crates/`](./crates).

```bash
git clone https://github.com/axiomwtf/poly.git
cd poly
cargo build --release
./target/release/poly --help
```

There is no `cargo install`-able binary yet; the crates.io crate is a source package. Once you have a `poly` binary, Poly-built projects work without Cargo: `poly install axiom-poly` downloads sources from crates.io, builds them with Poly and installs the result.

## Spec

Read the spec! The full specification is in [`SPEC.md`](./SPEC.md).

## License

MPL-2.0
