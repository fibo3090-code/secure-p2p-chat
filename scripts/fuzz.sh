#!/usr/bin/env bash
#
# Run a coverage-guided fuzz target.
#
#   ./scripts/fuzz.sh filename 300      # one target, 300 seconds
#   ./scripts/fuzz.sh                   # every target, 60 seconds each
#
# This is the deliberate, long-running half of the fuzzing story. The property
# tests in `core/tests/fuzz_parsers.rs` cover the same parsers, run on stable, and
# gate every pull request — that is what makes them useful. libFuzzer mutates
# toward new coverage instead, so it walks into branch combinations a random
# generator reaches only by luck, but it needs nightly and real time. Neither
# replaces the other.
#
# Two environment traps this script exists to absorb:
#
#   - `cargo fuzz` shells out to `cargo`, and on a machine where the distribution
#     ships its own `/usr/bin/cargo` that shadows the rustup proxy, the nested
#     call silently uses stable and the build fails on `-Z`. Prefixing
#     `~/.cargo/bin` fixes it; `+nightly` alone does not.
#   - `core/fuzz` is its own workspace on purpose, so a sanitizer build never
#     ends up in the path of an ordinary `cargo build`.

set -euo pipefail

cd "$(dirname "$0")/../core"

TARGET="${1:-}"
SECONDS_PER_TARGET="${2:-60}"

export PATH="$HOME/.cargo/bin:$PATH"
export RUSTUP_TOOLCHAIN=nightly

if ! rustup toolchain list 2>/dev/null | grep -q '^nightly'; then
    echo "error: fuzzing needs the nightly toolchain (rustup toolchain install nightly)" >&2
    exit 1
fi
if ! command -v cargo-fuzz >/dev/null 2>&1; then
    echo "error: cargo-fuzz is not installed (cargo install cargo-fuzz)" >&2
    exit 1
fi

run_one() {
    local name="$1"
    echo "── fuzzing ${name} for ${SECONDS_PER_TARGET}s ─────────────────────────"
    cargo fuzz run "$name" -- \
        -max_total_time="$SECONDS_PER_TARGET" \
        -print_final_stats=1
}

if [ -n "$TARGET" ]; then
    run_one "$TARGET"
else
    for name in $(cargo fuzz list); do
        run_one "$name"
    done
fi
