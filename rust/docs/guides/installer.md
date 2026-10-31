# Installing Cloner

`cloner` is a single Rust binary. There is no Ruby gem, Bundler, Node.js
runtime, `thor`, `ssh`, `rsync` or `scp` dependency at runtime.

The installer in [`rust/install.sh`](../../install.sh) installs the C/Rust build
toolchain if needed, builds the release binary, and copies it to
`~/.local/bin/cloner` (or another prefix).

## Quick install

```bash
git clone https://github.com/rs-pro/cloner.git
cd cloner
./rust/install.sh
```

If the clone does not contain `rust/` yet, switch to the Rust branch first:

```bash
git checkout rust
```

After installation:

```bash
cloner --version
cloner --help
```

If `~/.local/bin` is not on your `PATH`, add it:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

Add that line to `~/.bashrc`, `~/.zshrc` or the equivalent profile file.

## Installer options

```text
./rust/install.sh [OPTIONS]

  --prefix DIR     Install into DIR/bin (default: ~/.local/bin)
  --system         Install into /usr/local/bin using sudo
  --no-deps        Skip OS package installation
  --no-build       Install an existing target/release/cloner
  --no-install     Build only; do not copy the binary
  --dry-run        Print commands without running them
  -h, --help       Show help
```

Examples:

```bash
# Personal install, default
./rust/install.sh

# System-wide install
./rust/install.sh --system

# Custom location
./rust/install.sh --prefix "$HOME/opt/cloner"

# CI or an already provisioned machine
./rust/install.sh --no-deps

# Inspect what would happen
./rust/install.sh --dry-run
```

Re-running the installer upgrades the binary in place.

## What the installer does

1. Detects the OS and installs build dependencies with the native package
   manager.
2. Installs Rust via [rustup](https://rustup.rs) if `cargo` is missing.
3. Runs `cargo build --release --locked` in the `rust/` directory.
4. Copies `target/release/cloner` to `<prefix>/bin/cloner`.
5. Prints a `PATH` hint when needed and verifies `cloner --version`.

The Rust toolchain defaults to `stable`. Override it with:

```bash
CLONER_RUST_TOOLCHAIN=1.89.0 ./rust/install.sh
```

## Supported platforms

| Platform | Package manager | Build packages |
|---|---|---|
| Debian / Ubuntu | `apt-get` | `build-essential curl git ca-certificates pkg-config` |
| Fedora / RHEL 9+ | `dnf` | `gcc gcc-c++ make curl git ca-certificates pkgconf-pkg-config` |
| older RHEL / CentOS | `yum` | `gcc gcc-c++ make curl git ca-certificates pkgconfig` |
| Arch Linux | `pacman` | `base-devel curl git ca-certificates pkgconf` |
| openSUSE | `zypper` | `gcc gcc-c++ make curl git ca-certificates pkg-config` |
| Alpine | `apk` | `build-base curl git ca-certificates pkgconf` |
| macOS | Xcode CLT | `cc`, `make`, `git`, `curl` |

Linux and macOS are supported. On Windows use WSL2 with a Linux distribution.
The installer also accepts an unknown Linux distribution if `cc`, `make`, `git`
and `curl` are already present.

## Manual build

If you prefer not to run the installer:

```bash
git clone https://github.com/rs-pro/cloner.git
cd cloner/rust

# Debian/Ubuntu example
sudo apt-get update
sudo apt-get install -y build-essential curl git ca-certificates pkg-config

# Install Rust if needed
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
. "$HOME/.cargo/env"

cargo build --release --locked
install -d "$HOME/.local/bin"
install -m 0755 target/release/cloner "$HOME/.local/bin/cloner"
```

The minimum supported Rust version is **1.89** (declared in `Cargo.toml`).
All Rust dependencies come from crates.io; no local path dependencies are
required.

## Runtime requirements

The installer only installs **build** dependencies. At runtime:

- Docker with the Compose v2 plugin is needed for Compose-based projects.
- PostgreSQL/MySQL/MongoDB dump and restore tools must exist in the database
  containers (they normally do in the official images).
- If a project does not use Compose, the matching client tools
  (`pg_dump`/`pg_restore`, `mysqldump`/`mysql`, `mongodump`/`mongorestore`) must
  be installed locally.
- Project commands used in hooks (`go`, `bin/rails`, `bundle`, etc.) are only
  needed when those hooks run.
- The remote SSH server needs the SFTP subsystem (enabled by default in
  OpenSSH).

`cloner` itself needs no language runtime: QuickJS is statically embedded and
SSH/SFTP are implemented in Rust.

## Updating

```bash
cd cloner
git pull
./rust/install.sh
```

The binary is replaced atomically by `install`. Check the version with:

```bash
cloner --version
```

## Uninstalling

Personal install:

```bash
rm -f "$HOME/.local/bin/cloner"
```

System install:

```bash
sudo rm -f /usr/local/bin/cloner
```

The Rust toolchain and OS packages are shared; remove them separately if no
other project uses them.

## Troubleshooting

### `cargo: command not found` after installation

```bash
. "$HOME/.cargo/env"
export PATH="$HOME/.cargo/bin:$PATH"
```

Open a new shell, then re-run the installer.

### `linker 'cc' not found` / missing `make`

The C toolchain is incomplete. Install `build-essential` (Debian/Ubuntu),
`gcc gcc-c++ make` (Fedora/RHEL), or the Xcode Command Line Tools (macOS).

### macOS: `xcode-select: error: command line tools are not installed`

The installer starts the installer UI. Finish it, then re-run:

```bash
xcode-select --install
./rust/install.sh
```

### Permission denied when installing system-wide

Use `sudo ./rust/install.sh --system`, or install into the user prefix:

```bash
./rust/install.sh --prefix "$HOME/.local"
```

### `cloner: command not found` after a successful install

`~/.local/bin` is not on `PATH`. Add:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

Then open a new shell.

### Download is slow or the build machine is offline

Build on a connected machine, then copy the binary:

```bash
cargo build --release --locked
scp rust/target/release/cloner host:/usr/local/bin/cloner
```

`cloner` is a single executable; no extra files are required.

### musl / fully static binary

For a static Linux binary:

```bash
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

Copy `target/x86_64-unknown-linux-musl/release/cloner` instead.

## Next steps

- [Convert `dl.thor` to `dl.js`](thor-to-js-conversion.md)
- [CLI usage after `thor dl`](cli-usage.md)
- [DSL reference](dsl.md)
- [Operations and troubleshooting](operations.md)
