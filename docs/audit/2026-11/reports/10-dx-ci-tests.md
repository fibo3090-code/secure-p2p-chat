# Developer experience, CI and tests audit

## Scope covered

- **Workflows:** `.github/workflows/ci.yml` (947 lines), `audit.yml`, `codeql.yml`, `fuzz.yml`, `release.yml` and `.github/dependabot.yml`. For each I looked at the job graph, the change classifier, gating, caching, duplicated compilation, matrices, the apt steps, timeouts, concurrency, SHA pinning, permissions, secrets, and release provenance.
- **Configuration:** `.config/nextest.toml`, `deny.toml`, `.gitleaks.toml`, `.gitleaksignore`, `.github/codeql/config.yml`, `scripts/check-stale-advisories.sh`, `scripts/fuzz.sh` (by reference) and `run-codeql-local.ps1`.
- **Manifests and dependencies:** the root `Cargo.toml` and the four crate manifests. Profiles, features, `cargo tree --duplicates`, and per-crate compile cost from `cargo build --timings`.
- **Tests:** all 32 Rust test executables were built and run, each one timed. Every one of the 665 Rust tests was timed individually through libtest's `--report-time`, with `RUSTC_BOOTSTRAP=1` set at runtime. I also grepped the tests for sleeps, environment variables and fixed ports. On the frontend: vitest, eslint, `vite build`, and the Playwright configuration.
- **Onboarding documents:** `CONTRIBUTING.md`, `DEVELOPER_GUIDE.md` and `CLAUDE.md`, plus a check for repository tooling files (toolchain file, `.editorconfig`, hooks, a task runner).
- **The coordinator's extra items:** the frontend matrix running on three operating systems; the lack of a single local command equivalent to CI; RustSec advisories turning every PR red; and the gitleaks PEM fixture.

**Environment:** 4 vCPU, 15 GiB RAM, Linux, rustc/cargo 1.97.0, Node with `desktop/node_modules` already present.

**Tools that were missing, and what that left unmeasured:**
- `cargo-nextest`: nothing was installed. Tests were timed per binary instead.
- `cargo-llvm-cov`: no coverage numbers.
- Playwright browsers (`~/.cache/ms-playwright` is absent): smoke tests not run.
- `fd`: not installed, although `CLAUDE.md` says it is "installed globally".
- `/usr/bin/time`: not installed.
- clippy wall time was not measured, to save disk.
- `cargo-deny` 0.20.2 was available.

**Housekeeping:**
- The private target directory reached **18 GB** and was deleted at the end, with `rm -rf` on the exact path.
- `git status` was clean after the run. `vite build` reproduced the committed `desktop/dist/` byte for byte, so nothing tracked changed.
- The timings report and raw logs were kept in the audit session's scratch directory (not committed); rerun `cargo build --workspace --timings` to regenerate.

## Measurements

### Build times (cold, 4 cores, private `CARGO_TARGET_DIR`)

| Step | Wall time | Notes |
|---|---|---|
| `cargo build --workspace --timings` (clean) | **4 m 59 s** | 778 units, **1079 CPU-s** in total. Target dir after it: **6.5 GB** |
| `cargo test --workspace --no-run`, run after the build | **3 m 54 s** | 32 test executables. Target dir grew to **17 GB** |
| Rebuild with `opt-level=3` for 8 crypto crates (core lib tests only) | 2 m 38 s | One-time cost. See DX-01 |
| `cargo deny --all-features check` | 16 s | advisories, bans, licenses and sources all ok |
| `npx vitest run` | **27 s** wall (vitest reports 18.9 s) | 24 files, 287 tests, all passing |
| `npm run lint` | 4.2 s | |
| `vite build` | 4.8 s | |

### Slowest compilation units (`cargo build --timings`)

| Seconds | Unit | Seconds | Unit |
|---|---|---|---|
| 44.3 | gio 0.18 | 13.8 | syn **3**.0 |
| 35.5 | gtk 0.18 | 13.7 | p2pem-classic |
| 32.7 | p2pem-desktop | 12.8 | zbus 5 (via notify-rust) |
| 29.8 + 14.0 | tauri-utils (built twice: host and target) | 12.1 | image 0.25 |
| 21.1 | tokio | 11.2 | moxcms (image) |
| 20.7 | glib | 10.6 | ravif (AVIF encoder, image) |
| 15.3 | messenger-core | 6.6 / 6.1 / 2.5 | rav1e, exr, tiff (image) |
| 15.1 + 9.0 | regex-automata (built twice) | 8.7 | libsqlite3-sys build script (bundled C) |

The **image codec subtree adds about 89 CPU-s, roughly 8% of a clean build**, to encode one QR code as a PNG (see DX-04).

The dependency graph holds **541 unique crates** and **78 crates in duplicate versions**. Among them:
- three `syn` versions (1, 2 and 3);
- three `getrandom` versions;
- two whole RustCrypto generations: `digest` 0.10/0.11, `sha2` 0.10/0.11, `aead`, `cipher` and `chacha20`.

### Disk usage of one debug build plus one test build (17–18 GB)

| Item | Size |
|---|---|
| `debug/deps` | 12 GB |
| `debug/incremental` | 1.9 GB |
| `libp2pem_desktop_lib.a` (staticlib) | **1.38 GB**, plus a second **1.41 GB** copy built for the test profile |
| `libp2pem_desktop_lib.so` (cdylib) | **405 MB**, plus a second 415 MB copy |
| 18 client integration-test binaries | 225–253 MB each, **about 4.1 GB in total** |
| All 25 integration-test executables | 4.85 GB |

The shared `target/` directory filling to 30 GB is consistent with this. One debug build plus one test build already takes 17–18 GB, and clippy, a release build or rust-analyzer each add their own copy.

### Test binaries: sequential run, each using the default 4 threads (total **647.5 s, about 10 m 48 s**)

| Seconds | Binary | Seconds | Binary |
|---|---|---|---|
| **133.5** | messenger-core lib (unit tests) | 37.1 | client `file_transfer_e2e` |
| **100.8** | p2pem-classic lib | 24.9 | client `attack_trust_boundaries` |
| **96.1** | messenger-server lib | 15.3 | client `tui_loop_integration` |
| **61.0** | p2pem-desktop lib (bridge tests) | 15.1 | client `tui_integration_tests` |
| 55.2 | client `party_e2e` | 11.1 | client `reconnect_e2e` |
| 43.2 | core `session_e2e` | 7.5 | client `tui_command_tests` |
| 38.7 | core `relay_e2e` | under 5 each | the other 17 binaries |

- **Results:** 664 passed, 0 failed, 1 ignored. Plain `cargo test --workspace`, the documented fallback, also passed on every binary.
- **Where the time goes:** 2001 test-seconds in total. The **128 tests that take 5 s or more account for 1901 s, or 95%**.

### Slowest individual tests

| Seconds | Test |
|---|---|
| 63.5 | `tui::ui::tests::renders_every_overlay_without_panic` |
| 63.2 | `connection::tests::a_private_channels_messages_never_reach_a_non_member_over_the_tunnel` |
| 59.8 | `connection::tests::posted_message_is_broadcast_to_other_members` |
| 37.2 | `one_relay_brokers_a_second_session_after_the_first_ends` |
| 36.3 | `connection::tests::a_broadcast_arriving_mid_frame_does_not_break_the_connection` |
| 32.7 | `one_relay_brokers_two_pairings_at_once` |
| 28.7 | `tests::change_password_rotates_the_file_and_keeps_history_readable` (desktop) |
| 27.8 | `identity::tests::change_password_rewraps_the_same_key_and_keeps_the_history_key` |
| 25.8 | `identity::tests::save_replaces_atomically_and_leaves_no_temp_files` |
| 24.9 | `messages_keep_flowing_across_a_rekey` (`session_e2e`) |
| 19.5 | `core::crypto::tests::test_fingerprint_consistency` |

Almost every slow test spends its time in unoptimised RSA-2048 key generation (`core/src/identity/mod.rs:180,211`) or Argon2 at 64 MiB × t=3 (`core/src/identity/mod.rs:280-284`).

**Experiment** (DX-01): compiling eight crypto crates at `opt-level=3`, through `--config` with no file edits, changed these:

| Test or binary | Before | After |
|---|---|---|
| messenger-core lib, 191 tests | **133.4 s** | **8.2 s** (16× faster) |
| `test_fingerprint_consistency` | 19.5 s | 0.24 s |
| `change_password_rewraps…` | 27.8 s | 1.8 s |

### Frontend (vitest 5)

- 24 files, 287 tests, 18.9 s: 42% running tests, 38% setting up environments.
- Vitest's own hint: about 6.9 s faster with `isolate: false`. Isolation is defensible here because of the `localStorage` shim, so this is optional.
- The slowest single tests are about 400 ms each (`Settings.test.jsx`).

## Summary

**CI is unusually well engineered:**
- every action is pinned to a SHA;
- permissions are least-privilege;
- every job has a timeout;
- apt calls have bounded retries;
- one aggregate required check, with a fail-safe path classifier;
- release provenance is checked end to end.

The real costs are elsewhere:

1. **The test suite is slow because the crypto runs unoptimised in debug builds.** A four-line `[profile.dev.package]` change takes the slowest binary from 133 s to 8 s. Extrapolated, the suite's roughly 11 minutes, multiplied by three operating systems, should fall to about 2 minutes.
2. **The build and target directory are heavier than they need to be.** Contributors:
   - The desktop crate builds a 1.4 GB staticlib and a 0.4 GB cdylib that no desktop target uses.
   - The client has 18 separate 230 MB integration-test binaries.
   - Full debuginfo is on everywhere.
   - The `image` crate builds with all default codecs, AVIF/rav1e included.

   Together these explain the 30 GB `target/` and much of the 5 + 4 minute cold build.
3. **Process gaps let the outside world break every PR.** Today's RustSec advisories, yanked crates, and the floating `stable` toolchain with clippy `-D warnings` are all gated on every Rust PR, whatever the diff. There is no automated path to unblock them, which is how a `chore(deps)` commit had to ride inside feature PR #188.
4. **The classifier has a real hole.** The only Windows-capable check of the IPC invoke-key contract, and the CSP drift test, never run on a PR that touches only `desktop/src-tauri/`. That is exactly the PR that renames a command parameter.
5. **There is no single local command equivalent to CI.** The four documented lists of checks disagree with each other and with CI.

Recommended plan and expected gains are at the end.

## Findings — ordered by impact

### DX-01 — Crypto dependencies compile unoptimised in dev/test: 95% of test time is debug RSA and Argon2
- Severity: High
- Category: tests / build-time
- Status: VERIFIED (measured)
- Location: `Cargo.toml` has no `[profile.dev]` or `[profile.dev.package]` section; the only profile is `[profile.release]`. RSA keygen is at `core/src/identity/mod.rs:180,211`; Argon2 parameters are at `core/src/identity/mod.rs:280-284`. Affects `ci.yml:test` (three operating systems) and `coverage`.
- **Problem:**
  - 128 of 665 tests take 5 s or more and together make up 1901 of 2001 test-seconds.
  - The four library unit-test binaries take 133 s, 101 s, 96 s and 61 s.
  - In this environment the sequential suite takes 10 m 48 s. CI comments say "11 minutes" and "roughly twice as long" under coverage.
- **Impact:** about 11 minutes on each of three runners per Rust PR, roughly 22 minutes under coverage, and about 11 minutes for each contributor's local run. It also squeezes the 60-minute job timeouts on Windows.
- **Evidence:** with `--config 'profile.dev.package.{num-bigint-dig,rsa,argon2,blake2,sha2,num-integer,num-traits,crypto-bigint}.opt-level=3'`:
  - messenger-core lib tests: 133.4 s → **8.2 s**
  - `test_fingerprint_consistency`: 19.5 s → 0.24 s
  - identity change-password test: 27.8 s → 1.8 s
- **Recommendation:** add the following to the root `Cargo.toml`:
  ```toml
  [profile.dev.package.num-bigint-dig]
  opt-level = 3
  [profile.dev.package.rsa]
  opt-level = 3
  [profile.dev.package.argon2]
  opt-level = 3
  [profile.dev.package.blake2]
  opt-level = 3
  ```
  Add `sha2` and `crypto-bigint` too. Optionally, use `[profile.dev.package."*"] opt-level = 1` instead.

  Debug assertions in the workspace's own crates are unaffected. Separately, consider a lower-cost Argon2 parameter set for tests, behind a `cfg(test)` or test-only constructor, if it is not already done somewhere.
- **Expected gain:** the suite should drop from about 11 minutes to roughly 1–2 minutes per OS. This is a projection, not a measurement: only messenger-core was re-measured. It also shortens the coverage job by a similar factor. The one-time cost is about 2.5 minutes of extra dependency compile, which rust-cache then caches.
- Effort: S

### DX-02 — The change classifier skips the frontend contract tests when only `desktop/src-tauri/` changes
- Severity: High
- Category: ci / coverage
- Status: VERIFIED
- Location: `ci.yml:125-126` (`desktop/src-tauri/*) rust=true` only). Affected tests: `desktop/src/lib/invokeContract.test.js:22-27`, which reads `src-tauri/src/commands/*.rs`, and `desktop/src/lib/csp.test.js:30-32`, which reads `src-tauri/tauri.conf.json`.
- **Problem:**
  - The vitest invoke-contract test compares the keys `bridge.js` sends with the parameter names the Rust commands declare. It is the **only** platform-independent check of that contract: the Rust bridge tests are compiled out on Windows (`desktop/src-tauri/Cargo.toml` `[target.'cfg(not(windows))'.dev-dependencies]`).
  - Those Rust bridge tests use keys **hard-coded** in `tests.rs:354ff`. A PR that renames a Rust parameter and updates `tests.rs` passes them.
  - Such a PR touches only `desktop/src-tauri/`, so `frontend=false`, and the contract test never runs.
  - The same applies to a CSP change in `tauri.conf.json`, whose drift test is skipped.
- **Impact:** CLAUDE.md names this exact failure, an `invoke` that silently does nothing, as the root cause of "messages send but don't arrive". It can merge green.
- **Recommendation:** classify `desktop/src-tauri/src/commands/*` and `desktop/src-tauri/tauri.conf.json` (or simply all of `desktop/src-tauri/*`) as `rust=true; frontend=true`. Optionally, run just `npx vitest run src/lib/invokeContract.test.js src/lib/csp.test.js` inside a Rust job. That takes under 5 s with no browser.
- **Expected gain:** closes a silent-break path, at the cost of about one extra frontend job on src-tauri PRs.
- Effort: S

### DX-03 — `desktop` builds a staticlib and a cdylib it never uses (1.8 GB per profile, about 3.6 GB per build plus test), and debuginfo is full everywhere
- Severity: High (disk) / Medium (time)
- Category: build-time
- Status: VERIFIED for sizes; SUSPECTED for the exact time saving.
- Location: `desktop/src-tauri/Cargo.toml:13` (`crate-type = ["staticlib", "cdylib", "rlib"]`); no `[profile.dev] debug` setting in `Cargo.toml`.
- **Problem:**
  - `libp2pem_desktop_lib.a` is 1.38 GB and `.so` is 405 MB. Both are built again for the test profile (1.41 GB and 415 MB).
  - The `staticlib` and `cdylib` crate types exist only for Tauri mobile entry points. The project ships no mobile target, and the comment calls them "(future)".
  - `p2pem-desktop` is the third-slowest unit at 32.7 s. Producing a 1.4 GB archive and a 400 MB shared object is a large part of that, and it repeats on every incremental edit to the desktop crate.
  - Full debuginfo (the dev default, `debug = 2`) makes each test binary 225–253 MB.
- **Impact:** the 30 GB `target/` that filled the disk today. Longer link times on every edit. Larger rust-cache archives, which risk evicting each other (see DX-08).
- **Recommendation:**
  1. Set `crate-type = ["rlib"]`, or `["rlib", "cdylib"]` only when mobile work starts.
  2. Add `[profile.dev] debug = "line-tables-only"` and `[profile.dev.package."*"] debug = false`. Keeping backtraces with line numbers is enough for test failures.
  3. Optionally set `CARGO_PROFILE_DEV_DEBUG=0` in CI only.
- **Expected gain:** −3.6 GB from the crate types, and roughly −40–60% of `deps/` from debuginfo (typical; not measured here). Faster desktop links. Smaller CI caches.
- Effort: S

### DX-04 — `image` builds with all default codecs (AVIF/rav1e, EXR, TIFF…) to write one PNG; this re-creates an advisory acceptance and contradicts `deny.toml`
- Severity: Medium
- Category: dependency / build-time
- Status: VERIFIED
- Location: `Cargo.toml:108-109` (`qrcode = "0.14.0"`, `image = "0.25.9"`). The only use is at `client/src/app/chat_manager/invites.rs:137-144` (`Luma<u8>` → `ImageFormat::Png`). The stale claim is at `deny.toml:15` ("dropped … the `image` AVIF chain"). The acceptance is `deny.toml` `RUSTSEC-2024-0436` (paste).
- **Problem:**
  - `cargo tree -e features -i image` shows `avif`, `exr`, `tiff`, `dds`, `hdr`, `gif`, `jpeg`, `webp`, `qoi` and more all enabled through `default-formats`.
  - The timings show `ravif` 10.6 s, `moxcms` 11.2 s, `rav1e` 6.6 s, `exr` 6.1 s and `image` 12.1 s: about 89 CPU-s in total.
  - `paste`, the subject of an accepted "unmaintained" advisory, is reached **only** via `exr → pulp` and `rav1e`. So the advisory acceptance exists because of codecs nobody calls.
  - The AVIF chain is back in the tree, contrary to the `deny.toml` comment.
  - All of this also links into the shipped desktop app, because `p2pem-desktop` depends on `p2pem-classic`.
- **Recommendation:**
  - `image = { version = "0.25.9", default-features = false, features = ["png"] }`
  - `qrcode = { version = "0.14", default-features = false, features = ["image"] }`
  - Then run `cargo tree -i paste` and drop `RUSTSEC-2024-0436` if it no longer fires; `check-stale-advisories.sh` will insist on that anyway.
  - Check that `arboard`'s `image` use still builds with PNG only.
- **Expected gain:** about 70–89 CPU-s per cold build (about 7%), a smaller attack surface, and one fewer advisory acceptance.
- Effort: S

### DX-05 — World-state checks gate every PR: advisories, yanked crates and floating-stable clippy
- Severity: Medium
- Category: ci / dependency / process
- Status: VERIFIED
- Location:
  - `ci.yml:775-807`: `supply-chain` runs `cargo deny --all-features check` on every Rust PR.
  - `deny.toml` is v2, where yanked crates are errors.
  - `ci.yml:196-211`: `toolchain: stable` with `clippy -D warnings`.
  - No `rust-toolchain.toml` exists.
  - Evidence: commit `49a4e6e` "chore(deps): clear today's RustSec advisories" (ctutils, tokio, yanked async-executor/chacha20/clipboard-win, plus three new acceptances) merged inside feature PR #188 (`62b952f`).
- **Problem:**
  - The advisories, yanked and clippy checks answer "did the world change?", not "did this diff break anything?".
  - When RustSec publishes against a pinned dependency, every open Rust PR turns red at once, whatever its diff. The same happens when a crate is yanked, or when a new stable release adds a clippy lint.
  - The only way out is for some PR to carry an unrelated lockfile bump. That breaks CONTRIBUTING rule 3 ("keep changes scoped") and makes the dependency change hard to review or revert.
  - `audit.yml` already asks the advisories question daily and files an issue, so the PR gate adds no detection, only blast radius.
- **Recommendation:**
  1. Split the step:
     - `cargo deny check bans licenses sources` stays gating on every Rust PR.
     - `cargo deny check advisories` gates only when `Cargo.lock` or `deny.toml` changed, which a small classifier output can decide. Otherwise it runs `continue-on-error: true` with a `::warning::` annotation.
  2. Have `audit.yml` open a dedicated `chore(deps)` PR when it fails (`cargo update -p <crate>` for each fixable advisory), or enable Dependabot security updates, so unblocking is a one-click separate PR.
  3. Add a `rust-toolchain.toml` pinning, for example, `channel = "1.97.0"` with `components = ["rustfmt", "clippy"]`, and let Dependabot or Renovate bump it in its own PR. This also makes local and CI clippy agree.
- **Expected gain:** no more repository-wide red mornings. Dependency bumps get reviewed as dependency bumps.
- Effort: S–M

### DX-06 — Redundant compilation: `build-verification` duplicates what the Linux test leg already compiles
- Severity: Medium
- Category: ci / build-time
- Status: VERIFIED for the redundancy; SUSPECTED for the minutes (GitHub runner time not measured).
- Location: `ci.yml:600-663` (`cargo build --workspace --locked`, with its own apt install at `:636-655` and its own rust-cache at `:660`). Compare `ci.yml:388` (`cargo nextest run --workspace`, without `--locked`).
- **Problem:**
  - Each Rust PR compiles the workspace in five Linux jobs: `check` (clippy), `msrv` (check on 1.90), `test`, `build-verification`, and on main `coverage`. Each does its own apt install of WebKitGTK and keeps its own cache.
  - `build-verification` adds only two things: `--locked`, and the binaries. The binaries are already built by the test leg, because integration tests require them.
- **Recommendation:** add `--locked` to the test step (`cargo nextest run --workspace --locked --profile ci`) and to clippy, then delete `build-verification` (and remove it from `ci-success.needs`).
- **Expected gain:** one runner per Rust PR, roughly 5 min of build plus 1–5 min of apt on 4-vCPU runners (local cold build: 5 min), and one fewer cache entry.
- Effort: S

### DX-07 — The frontend job runs everything on three operating systems
- Severity: Medium
- Category: ci
- Status: VERIFIED (configuration); the timings are local.
- Location: `ci.yml:395-597`, with matrix `[ubuntu, windows, macos]`. Lint at `:456`, test at `:461`, build at `:465`, Playwright install at `:545-579`, smoke at `:583`. Also `desktop/package.json` `"smoke": "vite build && playwright test"`.
- **Problem:**
  - lint, test, build, Playwright Chromium install and smoke run in full on all three operating systems.
  - Lint is a pure AST pass and identical everywhere.
  - `npm run build` runs, and then `npm run smoke` builds again (`vite build &&`), so there are two builds per OS.
  - The expensive parts are `npm ci` on Windows and macOS, and the Playwright browser download (14-minute step budget). On Linux that download also runs `--with-deps`, which goes through apt.
  - The job's justification (path, timezone and `localStorage` differences) applies to `npm test` and smoke only.
  - The job timeout is 20 min, of which up to 14 is the Playwright step, so a cold cache on a slow Windows runner is tight.
- **Recommendation:**
  - Linux keeps the full suite: audit, lint, test, build, stale-bundle check, and smoke with both projects.
  - Windows and macOS run only `npm test` plus `npx playwright test --project=built` against a single build. Drop the separate `npm run build`, or add a `smoke:only` script that skips the rebuild.
  - Optionally, move the Windows and macOS legs to `push` on main plus `workflow_dispatch`, keeping Linux on PRs.
- **Expected gain:** about 2 Playwright installs and 2×(lint+build) fewer per frontend PR, and less wall time on the critical path. Minutes saved: SUSPECTED at 5–10 runner-minutes per PR.
- Effort: S

### DX-08 — Rust cache pressure: about 12 separate rust-cache entries, saved from every PR branch
- Severity: Medium
- Category: ci / build-time
- Status: SUSPECTED. The cache sizes on GitHub were not observable; the local target directory was 6.5–18 GB.
- Location: rust-cache at `ci.yml:205,288,365,660,725` and `fuzz.yml:76-79` (5 targets). None sets `save-if` or `shared-key`.
- **Problem:**
  - Each job gets its own key: check, msrv, test×3, build-verification, coverage, fuzz×5.
  - Each PR ref saves its own entries.
  - With debug artifacts several GB uncompressed (DX-03), the 10 GB per-repository Actions cache quota is likely to thrash, which means cold builds.
- **Recommendation:**
  - `save-if: ${{ github.ref == 'refs/heads/main' }}` on all Rust jobs, so PRs restore main's cache.
  - A `shared-key` between `check` and `msrv` where profiles allow.
  - Apply DX-03 to shrink every entry.
  - Consider `sccache` with the GHA backend only if cache hits stay poor after that.
- Effort: S

### DX-09 — No single local command equivalent to CI; the four documented check lists disagree
- Severity: Medium
- Category: onboarding / tooling
- Status: VERIFIED
- Location: `CONTRIBUTING.md:20-23,31-34,51-56`; `DEVELOPER_GUIDE.md:71,137-141`; `CLAUDE.md` "Development Commands". There is no `justfile`, `Makefile`, `scripts/ci-local.sh`, `.pre-commit-config.yaml`, `rust-toolchain.toml` or `.editorconfig`.
- **Problem:**
  - CI runs all of these: fmt, clippy, the `core/fuzz` check, MSRV `--locked`, nextest `--profile ci`, doctests, `--locked` build, cargo-deny plus `check-stale-advisories.sh`, actionlint, gitleaks, npm audit, lint, test, build, the stale-dist check, and smoke.
  - CONTRIBUTING lists 4 Rust plus 4 npm commands, and omits the fuzz check, deny, the stale-advisory script, `--locked` and the dist-staleness check.
  - The DEVELOPER_GUIDE release checklist (`:139`) says only `cargo test --workspace` and `npm test`: no clippy flags, no lint, no smoke.
  - Prerequisites are described as "the GTK/webkit dev packages CI installs", so a new Linux contributor has to read `ci.yml` to find `libgtk-3-dev libwebkit2gtk-4.1-dev libsoup-3.0-dev librsvg2-dev libayatana-appindicator3-dev`.
  - No Node version is stated (CI uses 22), nor any per-OS setup.
  - `CLAUDE.md` claims `fd` and `cargo-nextest` are "installed globally". Neither existed on this machine.
- **Impact:** a contributor's cold full check is about 5 min build + 4 min test build + 11 min tests + clippy + about 40 s frontend ≈ **20+ minutes** on 4 cores, and it is still not what CI runs. Failures surface only after a push.
- **Recommendation:**
  - Add one `scripts/ci-local.sh` (or a `justfile` with `ci`, `ci-rust`, `ci-frontend`, `ci-fast` recipes) that runs exactly CI's steps, skipping tools that are missing with a warning, as `check-stale-advisories.sh` already does.
  - Make CONTRIBUTING and DEVELOPER_GUIDE point to it instead of listing commands.
  - Add a per-OS prerequisites section: the apt list, Node 22, WebView2 and Xcode CLT.
  - Optionally add a pre-commit hook for `cargo fmt --check` and `gitleaks protect --staged`.
- **Expected gain:** one command and one source of truth. With DX-01, a full local run should take about 8 minutes cold and under 3 warm.
- Effort: S

### DX-10 — Secret scan: the CI copy installs gitleaks unverified, and PRs rescan the whole history every time
- Severity: Medium
- Category: ci / supply-chain
- Status: VERIFIED
- Location:
  - `ci.yml:893-900`: `curl … | sudo tar -xz -C /usr/local/bin gitleaks`, with no checksum and no `--proto '=https'`.
  - Compare `audit.yml:71-88`, which verifies SHA-256 and whose comment says the pipe-to-tar pattern was removed precisely because "the secret scanner was the one tool installed with no verification at all". The fix was applied to one of the two copies.
  - `ci.yml:871-904`: `fetch-depth: 0` plus `gitleaks detect` over the full history on every PR.
  - `.gitleaksignore`: one commit-pinned fingerprint for the PEM-looking fixture (`19e00af…:server/src/identity.rs:private-key:100`).
- **Problem:**
  - (a) A root-extracted, unverified binary runs in every CI run.
  - (b) Because every PR scans all history, a finding in any historical commit, like today's PEM fixture, turns every PR red until someone writes an ignore entry. The scan cost also grows with history.

  The fixture fix itself (build the string at runtime, plus a fingerprint for the historical commit) is correct and narrowly scoped.
- **Recommendation:**
  - Copy the checksum-verified install from `audit.yml` into `ci.yml`. Better, factor both into one composite action so they cannot drift again.
  - On `pull_request`, scan only the PR's commits: `gitleaks git --log-opts="origin/$BASE_REF..HEAD"`, or `gitleaks detect --log-opts=…`. Keep the full-history scan in `audit.yml` (daily) and on pushes to main.
  - Add `gitleaks protect --staged` as an optional pre-commit hook so a PEM-looking fixture is caught before it is committed.
- Effort: S

### DX-11 — `release.yml`: tauri-action is given `tagName`, `releaseName` and a token under a read-only permission block
- Severity: Medium
- Category: release
- Status: SUSPECTED. There are no tags or release runs in this clone to confirm against.
- Location: `release.yml:247-257` (`GITHUB_TOKEN`, `tagName`, `releaseName`). The workflow-level `permissions: contents: read` (`:17-18`) applies to `desktop`. The header comment (`:12-16`) says "The build matrices no longer upload anything to the release".
- **Problem:** given `tagName`, tauri-action creates or looks up the release and uploads the bundles itself. The draft created by `create-release` is not visible to a read-only token, and creating a release needs `contents: write`. So either:
  - the desktop legs fail with 403 on the first tagged release under this design; or
  - if the action tolerates the failure, it duplicates the upload path the design explicitly removed.

  Either way the inputs contradict the stated design.
- **Recommendation:** remove `tagName`, `releaseName` and `GITHUB_TOKEN` from the tauri-action step, so it builds only and `artifactPaths` is used as it already is. Then dry-run with `workflow_dispatch` on a `v0.0.0-test` tag in a fork.
- Related, all VERIFIED and Low:
  - The `tools` Linux leg runs on `ubuntu-latest` (`:326`) while desktop deliberately uses `ubuntu-22.04` (`:172`) for a low glibc baseline, so the terminal tools need a newer glibc than the app.
  - `macos-latest` builds only arm64 tools (labelled just "macos").
  - The release does not require CI to have passed on the tagged commit.
  - The toolchain is floating `stable` with no `SOURCE_DATE_EPOCH`, so attested builds are not reproducible.
- Effort: S

### DX-12 — Eighteen client integration-test crates: about 4.1 GB and 17 extra links per test build
- Severity: Low–Medium
- Category: build-time / tests
- Status: VERIFIED for sizes and counts.
- Location: `client/tests/*.rs` (21 files). The table above lists 18 binaries of 225–253 MB each. `history_scaling_probe.rs` is a whole binary for one `#[ignore]` probe (`:50`).
- **Problem:**
  - Each file is a separate crate. Each one links all of `p2pem-classic` + `messenger-core` + ratatui + tokio, and `party_e2e` links the server too.
  - Each binary is about 230 MB of debuginfo and costs a full link on every client change.
  - Several run in about 0 s: `persistence_tests`, `messaging_tests` and `integration_tests` at 0.0 s.
- **Recommendation:**
  - Consolidate into one harness, `client/tests/it/main.rs` with `mod persistence_tests; mod messaging_tests; …`. Keep `party_e2e` and others with process-global needs separate only if necessary; nextest isolates per test anyway.
  - Move the probe to `benches/` (with `harness = false`) or to an example.
- **Expected gain:** about −3.8 GB of disk and 16 fewer links per incremental test build. Likely 20–60 s per incremental `cargo test` (SUSPECTED; not measured).
- Effort: M

### DX-13 — Duplicated 20-line apt block ×5 in `ci.yml` and ×1 in `release.yml`, already drifting
- Severity: Low
- Category: ci / maintainability
- Status: VERIFIED
- Location: `ci.yml:157-194`, `258-278`, `317-355`, `620-655`, `690-714`; `release.yml:187-209`, which has a different package list including `libwayland-dev libxkbcommon-dev patchelf` (the egui leftovers that the ci.yml comment says "went away").
- **Problem:** five copies of the retry logic and package list must be kept in sync by hand. Each job also pays for its own `apt-get update` and install, about 1–5 minutes; `ci.yml:162` records a six-hour hang.
- **Recommendation:**
  - Make one composite action, `.github/actions/linux-deps`, used by every job.
  - Consider `awalsh128/cache-apt-pkgs-action` (pinned by SHA) to restore the .debs from cache.
  - Drop the wayland and xkb packages from the release job if Tauri does not need them.
- Effort: S

### DX-14 — Test hooks leak process-global environment variables and are compiled into release builds; the documented `cargo test` fallback relies on luck
- Severity: Low
- Category: flaky / tests
- Status: VERIFIED for the code; no failure was observed (`cargo test --workspace` passed here).
- Location:
  - `core/tests/session_e2e.rs:456-457` sets `P2PEM_TEST_KEEPALIVE_SECS`/`P2PEM_TEST_IDLE_TIMEOUT_SECS` with no lock and never resets them. Its comment relies on nextest's per-process isolation.
  - `core/src/network/session.rs:1322-1323` reads these variables in production code.
  - `relay_e2e.rs:206-226` correctly uses `ENV_LOCK`.
  - CONTRIBUTING (`:22,53`) offers `cargo test --workspace` as an equivalent.
  - `.config/nextest.toml` `retries = 1` is inherited by the `ci` profile.
- **Problem:**
  - Under plain `cargo test`, the other `session_e2e` tests in the same process inherit a 3-second idle timeout after that test runs. That is harmless today because keepalive is 1 s, but fragile.
  - A user-controlled environment variable can shorten idle and keepalive timers in the shipped binary.
  - A retried test passes CI green as FLAKY, which nothing forces anyone to read.
- **Recommendation:**
  - Make the knobs `#[cfg(any(test, feature = "test-hooks"))]`, or pass them through `SessionConfig`.
  - Remove the `set_var` calls.
  - Set `retries = 0` in `[profile.ci]`, or fail the job when nextest's JUnit output contains flaky tests.
  - State in CONTRIBUTING that nextest is required, not optional.
- Effort: S

### DX-15 — `run-codeql-local.ps1` does not reproduce CI's CodeQL
- Severity: Low
- Category: tooling
- Status: VERIFIED
- Location: `run-codeql-local.ps1:72` (`--command="cargo build"`, which builds only the default member, the client) and `:83` (`codeql/rust-queries`, default suite). Compare `codeql.yml:47-74`: three languages, `build-mode: none`, `config-file`, and `security-extended` with the test exclusions.
- **Problem:** the script is Windows-only (PowerShell plus `subst`), covers Rust only, ignores `.github/codeql/config.yml`, and uses a smaller query suite. So local results neither match CI nor have the noise filter that motivated advanced setup.
- **Recommendation:**
  - Use `--build-mode=none --codescanning-config=.github/codeql/config.yml` and `codeql/rust-queries:codeql-suites/rust-security-extended.qls`.
  - Add a POSIX twin, or move it to `scripts/`.
  - Or delete it and document `gh codeql`.
- Effort: S

### DX-16 — Stale or misleading comments that steer build decisions
- Severity: Low / Info
- Category: onboarding
- Status: VERIFIED
- Location:
  - `desktop/src-tauri/Cargo.toml:27-29` says the client "pulls in egui/ratatui transitively". egui is gone; ratatui, crossterm, arboard, clap, notify-rust/zbus and image still are.
  - `deny.toml:15` claims the image AVIF chain is gone (see DX-04).
  - The `ci.yml` coverage comment (`:665-668`) says "593 test functions"; this run counted 665 Rust tests plus 287 vitest tests.
  - `CLAUDE.md` "Preferred Tools" says `fd` and `cargo-nextest` are installed; neither was present.
- **Recommendation:** correct these. Extracting `ChatManager` into a UI-agnostic crate, as the comment proposes, would also remove ratatui, crossterm and the image codecs from the shipped desktop binary and its compile.
- Effort: S (comments) / L (crate split)

### DX-17 — Dependency duplication: two RustCrypto generations, three `syn`s, and `tokio` "full"
- Severity: Info
- Category: dependency
- Status: VERIFIED
- Location: `cargo tree --duplicates` shows 78 crates. `aes-gcm 0.11` and `ed25519/x25519-dalek 3` pull digest/aead/cipher 0.11/0.6/0.5, while `chacha20poly1305 0.10`, `hkdf 0.12` and `sha2 0.10` stay on the old generation. `syn 3` comes only via `natpmp → async-trait`; `syn 1` via `proc-macro-error`. `Cargo.toml:23` has `tokio` "full".
- **Impact:** a few extra compile seconds each (syn 3: 13.8 s), and two copies of the crypto primitives in the binary.
- **Recommendation:** move `chacha20poly1305`, `hkdf` and `sha2` to the matching new generation when they are released or stable. Replace `natpmp` or pin `async-trait`. Trim tokio features per crate; low priority.
- Effort: M

## Things done well

- **SHA pinning and least privilege:** every third-party action is pinned by commit SHA with a version comment. Workflow-level `contents: read`, with job-level grants only where needed (`issues: write`, `security-events: write`, `id-token: write`). `persist-credentials: false` on CI checkouts.
- **A fail-safe classifier and one gate:** unknown paths run everything; `if:`-gated jobs report `skipped` rather than disappearing; a single `ci-success` aggregate is the required check, and it treats `cancelled` as failure.
- **Bounded jobs:** every job has `timeout-minutes`. The apt retry with `timeout -k` plus `dpkg --configure -a` comes from a real incident. Playwright install timeouts are portable to macOS.
- **Concurrency:** superseded PR runs are cancelled while runs on main are not.
- **Release provenance:** draft-first; artifacts come only from this run; SHA256SUMS is itself attested; published bytes are re-downloaded and verified before publishing; no build cache in release.
- **Scheduled detection:** `audit.yml` (daily: deny, the stale-acceptance script, npm audit, gitleaks) and `fuzz.yml` (weekly, per-target cumulative corpus, `cache/save` under `always()`) file one issue each instead of a stream. `check-stale-advisories.sh` enforces that `deny.toml` stays minimal.
- **Tests that test the right thing:**
  - real end-to-end harnesses (two `ChatManager`s over loopback, an in-process community server, a relay);
  - the invoke-contract test parses both sides;
  - Playwright smoke has dev and built projects with a CSP-violation fixture;
  - the stale-`dist/` check is robust to whitespace and file lists;
  - `actionlint` and shellcheck run on workflows;
  - an MSRV job reads the floor from the manifest;
  - a `cargo check` of the separate fuzz workspace.
- **The frontend toolchain is fast and healthy:** vitest takes about 19 s for 287 tests, lint 4 s, build 5 s. `vite build` reproduced the committed `dist/` exactly.

## Prioritised optimisation plan

| # | Action | Findings | Effort | Expected gain |
|---|---|---|---|---|
| 1 | `[profile.dev.package]` `opt-level = 3` for num-bigint-dig, rsa, argon2, blake2, sha2 and crypto-bigint | DX-01 | S | Test run about 11 min → about 1–2 min per OS (core lib measured at 133 s → 8 s). Coverage job halves or better |
| 2 | Classify `desktop/src-tauri/*` as frontend too, or run the 2 contract tests in the Rust job | DX-02 | S | Closes the silent IPC-break hole |
| 3 | `crate-type = ["rlib"]`, plus `debug = "line-tables-only"` and dependencies at `debug = false` | DX-03 | S | −3.6 GB from the crate types, plus an estimated 40–60% of `deps/`; faster links; smaller caches |
| 4 | `image` PNG-only, `qrcode` without default features; drop the paste acceptance | DX-04 | S | About 70–89 CPU-s per cold build; smaller shipped binary |
| 5 | Make advisories non-blocking on PRs unless `Cargo.lock`/`deny.toml` changed; auto PR from `audit.yml`; pin the toolchain | DX-05 | S–M | No more repository-wide red from outside events |
| 6 | `--locked` on test and clippy; delete `build-verification` | DX-06 | S | −1 Linux runner (about 6–10 min) per Rust PR |
| 7 | Frontend: full suite on Linux; only `npm test` plus built-project smoke on Windows and macOS | DX-07 | S | −2 Playwright installs and −4 redundant lint/build runs per frontend PR |
| 8 | rust-cache `save-if` on main; composite actions for apt and gitleaks (with checksum); PRs scan only their own commits for secrets | DX-08, DX-10, DX-13 | S | Warmer caches, no supply-chain gap, less duplicated YAML |
| 9 | `scripts/ci-local.sh` or a `justfile`, one prerequisites section, optional pre-commit hook | DX-09 | S | One copy-pasteable command; a full local check at about 8 min cold after #1 |
| 10 | Fix tauri-action inputs; align the tools glibc baseline; dry-run the release | DX-11 | S | Release confidence |
| 11 | Consolidate client integration tests into one harness | DX-12 | M | −3.8 GB; 16 fewer links per test build |
| 12 | Gate the test environment knobs; `retries = 0` in CI | DX-14 | S | Removes a production test hook and hidden flakes |
