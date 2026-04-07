#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo is required"
  exit 1
fi

if ! command -v node >/dev/null 2>&1; then
  echo "node is required"
  exit 1
fi

time_cmd() {
  local label="$1"
  shift
  python3 - "$label" "$@" <<'PY'
import os, re, shutil, subprocess, sys, tempfile, time

label = sys.argv[1]
cmd = sys.argv[2:]
time_bin = shutil.which("time")
metric = None
wrapper = None
unit = None
if time_bin:
    if sys.platform == "darwin":
        wrapper = [time_bin, "-l"]
        metric = re.compile(r"^\s*(\d+)\s+maximum resident set size", re.M)
        unit = "bytes"
    else:
        wrapper = [time_bin, "-v"]
        metric = re.compile(r"Maximum resident set size \(kbytes\):\s*(\d+)")
        unit = "kbytes"

wrapped = cmd
if wrapper:
    wrapped = wrapper + cmd

start = time.perf_counter()
proc = subprocess.run(wrapped, capture_output=True, text=True)
elapsed = (time.perf_counter() - start) * 1000
mem = None
if metric:
    match = metric.search(proc.stderr or "")
    if match:
        mem = int(match.group(1))

if mem is not None and unit == "bytes":
    mem_mb = mem / (1024 * 1024)
    suffix = f", maxrss={mem_mb:.1f} MB"
elif mem is not None and unit == "kbytes":
    mem_mb = mem / 1024
    suffix = f", maxrss={mem_mb:.1f} MB"
else:
    suffix = ""
print(f"{label}: {elapsed:.1f} ms (exit={proc.returncode}{suffix})")
if proc.stdout:
    print(proc.stdout[:1200].rstrip())
if proc.stderr:
    print(proc.stderr[:1200].rstrip(), file=sys.stderr)
PY
}

echo "Building Rust binary..."
cargo build --release >/dev/null

echo
echo "A/B run: default"
time_cmd "node" node src/index.js
time_cmd "rust" ./target/release/ports-rs

echo
echo "A/B run: --all"
time_cmd "node" node src/index.js --all
time_cmd "rust" ./target/release/ports-rs --all

echo
echo "A/B run: ps"
time_cmd "node" node src/index.js ps
time_cmd "rust" ./target/release/ports-rs ps
