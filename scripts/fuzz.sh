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

# libFuzzer's default `-max_len` is 4096 bytes.
#
# Every interesting cap in these decoders sits above that: MAX_TEXT_MESSAGE_BYTES
# is 64 KiB, TEXT_CHUNK_BYTES is 48 KiB, FILE_CHUNK_SIZE is 64 KiB. At the
# default the mutator can never build an input that reaches the branch on the far
# side of a length check, so the whole "what happens at and past the cap" half of
# each decoder was unreachable no matter how long the fuzzer ran — which is why
# the protocol target could not find the amplification bug that its own
# round-trip contract describes.
#
# 96 KiB clears the largest cap with room for the header fields in front of it.
MAX_LEN="${P2PEM_FUZZ_MAX_LEN:-98304}"

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

# Corpora live here and are reused across runs. libFuzzer's coverage feedback is
# cumulative, so a corpus thrown away after every run makes each one start from
# nothing — which for a decoder guarded by length checks means starting from
# nothing every time. Gitignored, because a corpus is generated data.
CORPUS_ROOT="fuzz/corpus"

# Tracked starting material, one directory per target. Copied into the corpus
# before each run rather than passed as a second corpus directory, so libFuzzer
# is free to minimise and extend them in place without rewriting tracked files.
#
# They exist because the accumulated corpus does not: `core/fuzz/corpus/` is
# gitignored, so every fresh checkout started from nothing. For the bincode
# targets that is not merely slow but hopeless — an enum variant is a
# little-endian u32, and four random bytes are essentially never a valid one, so
# the whole budget goes on being rejected at the first field. See
# `core/fuzz/seeds/README.md`.
SEED_ROOT="fuzz/seeds"

run_one() {
    local name="$1"
    local corpus="${CORPUS_ROOT}/${name}"
    mkdir -p "$corpus"
    if [ -d "${SEED_ROOT}/${name}" ]; then
        cp -n "${SEED_ROOT}/${name}"/* "$corpus"/ 2>/dev/null || true
    fi
    echo "── fuzzing ${name} for ${SECONDS_PER_TARGET}s ─────────────────────────"
    cargo fuzz run "$name" "$corpus" -- \
        -max_total_time="$SECONDS_PER_TARGET" \
        -max_len="$MAX_LEN" \
        -print_final_stats=1
}

if [ -n "$TARGET" ]; then
    run_one "$TARGET"
else
    # `set -e` would stop the sweep at the first target that finds something,
    # so the targets after it never run at all — the run that finally finds a
    # bug is exactly the run you most want the rest of the results from. Each
    # failure is recorded and reported together at the end instead.
    failed=()
    for name in $(cargo fuzz list); do
        if ! run_one "$name"; then
            echo "!! ${name} reported a failure; artifacts are under core/fuzz/artifacts/${name}" >&2
            failed+=("$name")
        fi
    done
    if [ ${#failed[@]} -gt 0 ]; then
        echo >&2
        echo "error: ${#failed[@]} target(s) failed: ${failed[*]}" >&2
        exit 1
    fi
fi
