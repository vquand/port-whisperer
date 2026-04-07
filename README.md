# port-whisperer

**A Rust-first CLI to inspect listening ports and developer processes.**

This repository ports the original `port-whisperer` idea to Rust so it can ship as a native binary with lower runtime overhead and simpler multi-platform distribution.

## Credit

The original project was created by **Larsen Cundric**.

- Original package: `port-whisperer`
- Original repository: `https://github.com/larsencundric/port-whisperer`

This repository keeps that functionality as the reference baseline in [`src/`](/Users/doquan/GitHub/port-whisperer/src), and adds a Rust implementation in [`rust-src/main.rs`](/Users/doquan/GitHub/port-whisperer/rust-src/main.rs).

## Why Rust?

The Node.js version works, but it pays for a JS runtime on every invocation. For a CLI that is launched repeatedly, that cost is visible.

Rust is a better fit here because it gives:

- Lower memory overhead for short-lived CLI runs.
- Lower startup overhead on heavier commands.
- A single native binary instead of a Node runtime plus package dependencies.
- Easier packaging for macOS, Linux, and Windows.
- A clearer base for future system-level integrations.

The goal of this port is not to change the product. The goal is to keep the same workflow while making distribution and runtime characteristics better.

## Implementations

This repo currently contains two implementations:

- `src/`: original Node.js baseline
- `rust-src/main.rs`: Rust port, builds to `ports-rs`

The Rust CLI currently covers:

- port table
- `--all`
- `ps`
- `<port>` detail
- `kill`
- `clean`
- `watch`

## Resource Comparison

Measurements below were taken on **April 7, 2026** on the current macOS workspace using the in-repo A/B harness in [`scripts/ab-test.sh`](/Users/doquan/GitHub/port-whisperer/scripts/ab-test.sh).

The numbers are for the same commands run against the Node baseline and the Rust binary.

| Command | Version | Start time | CPU time (user + sys) | Max RSS |
|---------|---------|------------|------------------------|---------|
| default | Node | 226.9 ms | 170 ms | 50.1 MB |
| default | Rust | 271.9 ms | 80 ms | 27.0 MB |
| `--all` | Node | 199.9 ms | 160 ms | 50.7 MB |
| `--all` | Rust | 104.0 ms | 70 ms | 26.5 MB |
| `ps` | Node | 240.2 ms | 230 ms | 56.3 MB |
| `ps` | Rust | 154.9 ms | 140 ms | 9.5 MB |

What this means in practice:

- Rust used much less RAM in every measured command.
- Rust used less CPU time in every measured command.
- Rust was faster for `--all` and `ps`.
- Rust was slightly slower on the default filtered command in this run, but still used much less memory and CPU.

These are directional benchmarks for this machine and current process set, not a universal claim for all environments.

## Install

### Prerequisites

- Rust toolchain (`cargo`, `rustc`)
- On macOS and Linux: system tools such as `lsof`, `ps`, and optionally `docker`

### Build

From the repository root:

```bash
cargo build --release
```

The binary will be available at:

```bash
./target/release/ports-rs
```

### Optional: keep the Node baseline for comparison

```bash
npm ci
```

## Usage

### Show dev ports

```bash
./target/release/ports-rs
```

### Show all listening ports

```bash
./target/release/ports-rs --all
```

### Show all developer processes

```bash
./target/release/ports-rs ps
```

### Inspect one port

```bash
./target/release/ports-rs 3000
```

### Kill by port or PID

```bash
./target/release/ports-rs kill 3000
./target/release/ports-rs kill 42872
./target/release/ports-rs kill -f 3000
```

### Clean orphaned or zombie dev processes

```bash
./target/release/ports-rs clean
```

### Watch for port changes

```bash
./target/release/ports-rs watch
```

## A/B Testing

To compare the Node baseline with the Rust binary:

```bash
npm ci
./scripts/ab-test.sh
```

The script reports:

- elapsed start time
- CPU usage via `user + sys`
- maximum resident memory

## How it works

Both versions follow the same basic model:

1. Find listening TCP ports.
2. Batch-fetch process metadata.
3. Resolve working directories where possible.
4. Detect framework and project metadata.
5. Present a CLI view for ports or processes.

The Rust version still uses platform-native system utilities and OS process data, but removes the Node runtime from the execution path.

## Platform Support

| Platform | Status |
|----------|--------|
| macOS | Supported in Node and Rust |
| Linux | Partial support in Rust |
| Windows | Partial support in Rust |

## License

[MIT](LICENSE)
