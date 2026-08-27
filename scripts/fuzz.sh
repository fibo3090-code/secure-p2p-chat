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

# ── Input size ───────────────────────────────────────────────────────────────
#
# libFuzzer defaults to 4096 bytes. Every interesting cap in these decoders sits
# well above that — MAX_TEXT_MESSAGE_BYTES is 64 KiB, TEXT_CHUNK_BYTES is 48 KiB
# — so at the default the fuzzer cannot reach the branches worth fuzzing no
# matter how long it runs. That is not a theoretical gap: the round-trip bug in
# the text decoder (lossy UTF-8 expanding one byte to three, so a frame inside
# the wire cap decoded past it) lives strictly above 64 KiB, and `protocol_frame`
# asserted exactly the property it violated while being unable to generate an
# input that would show it.
#
# 128 KiB clears the largest of those caps with room to overshoot it.
MAX_LEN="${P2PEM_FUZZ_MAX_LEN:-131072}"

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

failed=()

run_one() {
    local name="$1"
    # cargo-fuzz accumulates a corpus here between runs; `fuzz/seeds/` is the
    # tracked starting material, so a fresh checkout does not begin from nothing
    # — the corpus directory itself is gitignored.
    local corpus="fuzz/corpus/${name}"
    local seeds="fuzz/seeds/${name}"
    mkdir -p "$corpus"
    # `2>/dev/null || true` here would swallow a real failure — an unreadable
    # seed, or a corpus directory that is not writable — and the run would then
    # fuzz from nothing while still reporting "all targets finished cleanly".
    # The only error worth ignoring is the empty-glob case, which `compgen`
    # separates out.
    if [ -d "$seeds" ] && compgen -G "$seeds/*" > /dev/null; then
        cp -n -- "$seeds"/* "$corpus"/
    fi

    echo "── fuzzing ${name} for ${SECONDS_PER_TARGET}s (max_len=${MAX_LEN}) ──────"

    # A crash in one target must not skip the rest. `set -e` aborted the whole
    # loop on the first failure, so later targets never ran at all — and the
    # artifact for the one that did fail landed in a gitignored directory with
    # nothing said about where. Record it, keep going, report at the end.
    if cargo fuzz run "$name" "$corpus" -- \
        -max_total_time="$SECONDS_PER_TARGET" \
        -max_len="$MAX_LEN" \
        -print_final_stats=1; then
        return 0
    fi

    failed+=("$name")
    echo "!! ${name} failed. Reproducers (gitignored, so copy them out):" >&2
    ls -1 "fuzz/artifacts/${name}" 2>/dev/null | sed "s|^|   core/fuzz/artifacts/${name}/|" >&2 || true
    echo "   Re-run one with: cargo fuzz run ${name} core/fuzz/artifacts/${name}/<file>" >&2
    return 0
}

if [ -n "$TARGET" ]; then
    run_one "$TARGET"
else
    for name in $(cargo fuzz list); do
        run_one "$name"
    done
fi

if [ "${#failed[@]}" -gt 0 ]; then
    echo
    echo "fuzzing found crashes in: ${failed[*]}" >&2
    exit 1
fi

echo
echo "all targets finished cleanly"
