# Fuzzing seed corpus

Tracked starting material for `./scripts/fuzz.sh`. One directory per target;
`scripts/fuzz.sh` copies these into `core/fuzz/corpus/<target>/` before each run.

They are here because the accumulated corpus is not. `core/fuzz/corpus/` and
`core/fuzz/artifacts/` are gitignored, so every fresh checkout — and every CI
machine — started libFuzzer from nothing. That is a slow start for the parsers
that matter most and an impossible one for two of them:

* **`party_frame`** and **`identity_proof`** are bincode, which encodes an enum
  variant as a little-endian `u32`. Four random bytes are essentially never a
  valid variant index, so a run from an empty corpus spends its whole budget
  being rejected at the first field. These seeds are produced by the real
  encoder, so every one of them decodes.
* **`protocol_frame`** and **`filename`** have their interesting branches above
  48–64 KiB (see the `-max_len` note in `scripts/fuzz.sh`). The seeds sit at and
  just past those caps — including the invalid-UTF-8 text frame whose 3×
  expansion broke the round-trip property, and the bidi-override filename the
  `filename` target's docstring is about.
* **`framing`** is all about the four-byte length prefix: empty, short,
  zero-length, exactly at `MAX_PACKET_SIZE`, one byte past it, and on the 64 KiB
  chunked-read boundary.

Keep them small and keep them meaningful — a seed that is not near a boundary is
just a slower way to start.
