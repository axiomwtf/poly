# Poly

**One build system for Rust, C/C++ and Go**

Poly is a build system and package manager for Rust, C/C++ and Go, with one manifest (`poly.ini`), one lock file (`poly.lock`), and one command set. It invokes `rustc`, C/C++ compilers, and the Go toolchain directly — no Cargo, CMake, or Make involved in user-project builds.

> **Status:** All six spec phases implemented.

## Features

- **Unified manifest**: Single `poly.ini` for all three languages
- **Direct compiler invocation**: `rustc`, `gcc`/`clang`/MSVC, `go` — never Cargo for user projects
- **Full Rust dependency compilation**: transitive deps, features, optional deps, renamed deps, proc macros, build scripts with `links`/`DEP_*` propagation, target-specific deps
- **Source-usage analysis**: only crates actually referenced in `use`/paths are compiled as direct externs
- **Cross-compilation**: Linux + Windows binaries from a single `poly build` (MinGW toolchain for Windows)
- **Unit-level caching**: unchanged dependency units are skipped on rebuild
- **Lock file**: `poly.lock` pins resolved versions and checksums
- **VFSX packaging**: per-platform `.vfsx.tar.zst` archives plus a combined installable package (auto-created; `--no-package` to skip)
- **Signing & verification**: ed25519 signatures via the external `vfsx` tool
- **crates.io publish/install**: `poly publish` uploads a `.crate` (stub `Cargo.toml` + full sources + `poly.ini`/`poly.lock`); `poly install` downloads, compiles with Poly, and installs the result
- **Developer tooling**: `test`, `lint`, `fmt`, `doc`, `ci`, `migrate`, `completion`, `cache`, `tree`, `outdated`, `audit`, `sbom`, `profile`, `ide`, `release`, `review`, `diff`, `flamegraph`, `stats`, `env`
- **Auth**: `poly login`/`poly logout` store registry tokens in `~/.config/poly/credentials.toml`

## Installation

There is no `cargo install`-able binary yet (the crates.io crate is a source package). Build from source:

```bash
git clone https://github.com/axiomwtf/poly.git
cd poly
cargo build --release   # Cargo is only used to build Poly itself during development
./target/release/poly --help
```

Once you have a `poly` binary, Poly-built projects work without Cargo:

```bash
poly install axiom-poly   # downloads sources from crates.io, builds with Poly, installs
```

## Usage

### Initialize a project

```bash
poly init --name my-project
```

### Build

```bash
poly build
poly build --release
poly build --no-package   # skip the combined .vfsx.tar.zst package
```

Builds all targets for **both Windows and Linux** — there's no flag to pick one OS.

**Output** goes to `dist/{profile}/`:

```
dist/
├── debug/
│   ├── linux/x86_64-unknown-linux-gnu/my-app
│   ├── windows/x86_64-pc-windows-gnu/my-app.exe
│   ├── poly-linux.debug.vfsx.tar.zst     ← per-platform archives
│   ├── poly-windows.debug.vfsx.tar.zst
│   └── deps/                            ← cached dependency artifacts
└── my-app-1.0.0.vfsx.tar.zst            ← combined installable package
```

### Package / sign / verify / install

```bash
poly package            # combined dist/{name}-{version}.vfsx.tar.zst
poly sign               # sign archives with the vfsx identity
poly verify             # verify signatures
poly install my-app     # from crates.io (builds from source) or a Poly registry
poly install my-app --local dist/my-app-1.0.0.vfsx.tar.zst
poly uninstall my-app
```

### Publish

```bash
poly login              # paste a crates.io API token (stored locally)
poly publish            # publishes every rust:* target in [publish] targets
```

Each Rust target becomes a `.crate` on crates.io containing the full project sources, `poly.ini`, `poly.lock`, and a stub `Cargo.toml` (registry compatibility only — `poly.ini` is the source of truth). Crate naming: `[publish] crateName` + target suffix for multi-target projects, or the target id by default.

### Quality tooling

```bash
poly test [target]      # compile & run #[test] targets
poly lint [target]      # rustc/cppcheck-style checks
poly fmt                # rustfmt / clang-format / gofmt
poly doc                # generate docs into doc/
poly ci <provider>      # generate CI workflow
poly completion <shell> # shell completions
poly cache              # cache stats / clean
poly tree / outdated    # dependency views
poly audit / sbom       # security audit / SBOM generation
```

## Configuration

### poly.ini

```ini
[project]
name = my-app
version = 1.0.0
edition = 2024
description = "..."
license = MIT
repository = https://github.com/you/my-app

[target.app]
language = rust
type = bin
sources = src/main.rs
edition = 2024

[target.crypto]
language = c
type = static-lib
sources = src/crypto/*.c
compiler = clang
flags = -O3, -Wall

[dependencies.rust]
serde = 1.0|derive
tokio = 1.40|full

[dependencies.c]
openssl = 3.0|poly

[dependencies.go]
github.com/gorilla/mux = 1.8

[dependencies.workspace.app]    # path deps between workspace targets
my-lib = path

[build.default]
targets = rust:app, c:crypto

[publish]
crateName = my-org-my-app      # optional crates.io name override
targets = rust:app             # defaults to build targets
platforms = x86_64-linux, x86_64-windows
registry = https://...         # optional custom Poly registry for the vfsx package
tokenEnv = MY_REGISTRY_TOKEN

[hooks]
preBuild = "echo building"
postBuild = "echo done"
```

### Conventions

- `[section.sub]` makes hierarchy; commas make arrays; `|` adds metadata (version|features)
- Keys are camelCase (`includeDocs`, `runOnBuild`, `crateName`, `tokenEnv`)
- Dependency specs: `version`, `version|feat1,feat2`, `path` (workspace member)

## Architecture

Cargo workspace:

- `poly-cli` - CLI, crates.io publish/install, hooks
- `poly-core` - dependency compiler (unit graph, cache, lockfile)
- `poly-config` - `poly.ini` parser
- `poly-resolver` - version resolution
- `poly-rust` / `poly-c` / `poly-go` - language drivers
- `poly-linker` - cross-language linking
- `poly-vfsx` - external `vfsx` tool wrapper
- `poly-registry` - registry HTTP client
- `poly-package` - packager & installer

## Honest limitations

- The resolver is greedy latest-match, not a full SAT solver — exotic version conflicts can resolve incorrectly
- C/C++ and Go drivers work but are less battle-tested than Rust
- Some advanced commands (`audit`, `flamegraph`, `toolchain install`, plugins) are functional but basic
- crates.io packages are source distributions; `poly install` compiles them locally
- Git dependencies, `[patch]`/`[replace]`, and dev-dependencies are not yet supported

## License

MPL-2.0

## Links

- [Axiom](https://github.com/axiomwtf) - Studio behind Poly
- [VFSX](https://github.com/axiomwtf/vfsx) - Signed archive format used by Poly
- Spec: `polyspec.md` in the [axiom](https://github.com/axiomwtf/axiom) repo
