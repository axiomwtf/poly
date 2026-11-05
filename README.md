# Poly

**One build system for Rust, C/C++ and Go**

Poly is a build system and package manager for Rust, C/C++ and Go, with one manifest (`poly.ini`), one lock file (`poly.lock`), and one command set.

> **Status:** Phase 1 (Core MVP) - In development

## Features

- **Unified manifest**: Single `poly.ini` file for all three languages
- **Direct compiler invocation**: Calls `rustc`, `gcc`/`clang`, and `go tool` directly
- **Dependency resolution**: Basic SAT-based resolver for cross-language dependencies
- **Lock file**: Unified `poly.lock` for all dependencies
- **Task graph**: Build orchestration with dependency tracking
- **Caching**: Local build cache (basic implementation)
- **Multi-target builds**: Support for libraries, binaries, and static libraries

## Installation

```bash
cargo install --path crates/poly-cli
```

Or build from source:

```bash
cargo build --release
```

The binary will be at `target/release/poly`.

## Usage

### Initialize a project

```bash
poly init --name my-project
```

This creates:
- `poly.ini` - project manifest
- `src/lib.rs` - basic Rust library

### Build

```bash
poly build
```

Builds all targets defined in `poly.ini`.

### Validate configuration

```bash
poly validate
```

Checks that `poly.ini` is valid.

### Generate lock file

```bash
poly lock
```

Resolves dependencies and generates `poly.lock`.

### Clean

```bash
poly clean
```

Removes build artifacts.

### Add/Remove dependencies

```bash
poly add rust:serde
poly remove go:github.com/gorilla/mux
```

(Full implementation pending)

## Configuration

### poly.ini

Example configuration:

```ini
[project]
name = my-app
version = 1.0.0
edition = 2024

[target.lib]
language = rust
type = lib
sources = src/lib.rs

[target.crypto]
language = c
type = static-lib
sources = src/crypto/*.c, src/crypto/*.h
compiler = clang
flags = -O3, -Wall

[target.server]
language = go
type = bin
sources = cmd/server/*.go
module = github.com/user/my-app/server
cgo = true

[dependencies.rust]
serde = 1.0
tokio = 1.0|full

[dependencies.c]
openssl = 3.0|poly
libz = 1.2|system

[dependencies.go]
github.com/gorilla/mux = 1.8

[build.default]
targets = rust:lib, c:crypto, go:server
```

### Conventions

1. **Dots make hierarchy**: `[target.lib]` is target `lib`, `[build.profiles.release]` is nested
2. **Commas make arrays**: `members = lib, bin` and `flags = -O3, -Wall`
3. **Pipes add metadata**: `tokio = 1.0|full` means version 1.0 with `full` features
4. **Keys are camelCase**: `trustStore`, `includeDocs`, `runOnBuild`

### Naming rules

- Config keys must be camelCase
- Section names are lowercase with dots for hierarchy
- Dependency names are exempt (kept as published)
- Poly warns on non-camelCase keys

## Architecture

Poly is organized as a Cargo workspace with the following crates:

- `poly-cli` - Command-line interface
- `poly-core` - Task graph, caching, and execution
- `poly-config` - `poly.ini` parser
- `poly-resolver` - Dependency resolution (SAT solver)
- `poly-rust` - Rust driver (direct `rustc`)
- `poly-c` - C/C++ driver (`gcc`, `clang`, `msvc`)
- `poly-go` - Go driver (`go tool compile`/`link`)

## Roadmap

### Phase 1: Core (MVP) ✅
- `poly.ini` parser
- `poly.lock` generation
- Dependency resolver (SAT solver)
- Basic drivers for rustc, gcc and go
- Simple task graph execution
- Basic cache

### Phase 2: Language support
- Full Rust driver (features, cfg, build scripts)
- Full C/C++ driver (header scanning, linking)
- Full Go driver (modules, CGO)
- Unified linker orchestration

### Phase 3: Integrations and packaging
- VFSX signing and verification
- Package registry client
- Single-binary packaging

### Phase 4: Developer experience
- Test runner
- Linting and formatting
- Documentation generation
- Shell completion
- IDE integration

### Phase 5: Advanced features
- Toolchain management
- Cross-compilation
- CI/CD generation
- Migration tools
- Plugin system

### Phase 6: Registry and ecosystem
- poly.dev registry
- Package analytics
- Private registries
- Binary installation and distribution

## License

MPL-2.0

## Contributing

Contributions are welcome! Please see the [spec](../axiom/polyspec.md) for details.

## Links

- [Axiom](https://github.com/axiomwtf) - Studio behind Poly
- [VFSX](https://github.com/axiomwtf/vfsx) - Signed archive format used by Poly
- [Poly spec](../axiom/polyspec.md) - Full specification
