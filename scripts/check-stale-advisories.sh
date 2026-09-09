#!/usr/bin/env bash
#
# Fail if `deny.toml` accepts an advisory that no longer applies.
#
# `deny.toml` says it itself: "A stale entry is worse than no entry — it silently
# suppresses a future finding on a crate we have since started using
# differently." That is exactly right, and until now nothing enforced it. The
# list only ever grows, entries outlive the dependency that justified them, and
# the day one of those crates comes back in a different role the advisory is
# suppressed before anyone sees it.
#
# cargo-deny has no built-in report for unused advisory ignores (checked against
# 0.19.9), so this derives it: run the check once with the ignore list emptied,
# collect every advisory that actually fires, and compare against what is listed.
# Anything listed but not firing is dead weight and gets removed.
#
# Exits non-zero with the offending IDs, so CI reports them by name.

set -euo pipefail

cd "$(dirname "$0")/.."

if ! command -v cargo-deny >/dev/null 2>&1; then
    echo "cargo-deny is not installed; skipping the stale-advisory check" >&2
    exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"; [ -f deny.toml.stale-check-backup ] && mv deny.toml.stale-check-backup deny.toml' EXIT

listed="$work/listed.txt"
firing="$work/firing.txt"

# Only what the `ignore = [...]` array actually accepts.
#
# This used to grep the whole file, which meant a *comment* mentioning an
# advisory ID counted as accepting it — so writing down why an entry had been
# removed re-created the very failure that removing it was meant to fix. A
# comment accepts nothing; the array is the thing with an effect, so the array
# is what to read.
python3 - > "$listed" <<'PARSE'
import re
s = open("deny.toml", encoding="utf-8").read()
block = re.search(r"ignore\s*=\s*\[(.*?)\n\]", s, re.S)
body = block.group(1) if block else ""
# Strip comments inside the array too: an entry can be commented out in place,
# and one that is commented out is not in force either.
body = re.sub(r"#[^\n]*", "", body)
for advisory in sorted(set(re.findall(r"RUSTSEC-\d{4}-\d{4}", body))):
    print(advisory)
PARSE

# Temporarily neutralise the ignore list so every advisory reports itself.
cp deny.toml deny.toml.stale-check-backup
python3 - <<'PY'
import re
s = open("deny.toml", encoding="utf-8").read()
s = re.sub(r"ignore = \[.*?\n\]", "ignore = []", s, flags=re.S)
open("deny.toml", "w", encoding="utf-8").write(s)
PY

# The check is *expected* to fail here — we want its findings, not its verdict.
cargo deny --all-features check advisories 2>&1 \
    | grep -oE 'RUSTSEC-[0-9]{4}-[0-9]{4}' | sort -u > "$firing" || true

mv deny.toml.stale-check-backup deny.toml

stale="$(comm -23 "$listed" "$firing")"

if [ -n "$stale" ]; then
    echo "::error::deny.toml accepts advisories that no longer apply:"
    echo "$stale" | sed 's/^/  /'
    echo
    echo "Remove them. A stale acceptance suppresses a real future finding on the"
    echo "same advisory, which is the failure mode deny.toml's own comment warns"
    echo "about."
    exit 1
fi

echo "deny.toml: $(wc -l < "$listed") accepted advisories, all still applicable."

# Also surface anything firing that is *not* listed. `cargo deny check` fails on
# those anyway, but naming them here makes the cause obvious rather than leaving
# someone to diff two lists by hand.
unlisted="$(comm -13 "$listed" "$firing")"
if [ -n "$unlisted" ]; then
    echo "note: advisories firing without an entry (cargo-deny will fail on these):"
    echo "$unlisted" | sed 's/^/  /'
fi
