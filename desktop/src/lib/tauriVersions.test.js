// The Tauri crate and its JS API must stay on the same minor release.
//
// `tauri build` refuses to run when they differ ("Found version mismatched
// Tauri packages"), and nothing else in CI runs `tauri build`: the Rust jobs
// use cargo, the frontend jobs use vite. So a Dependabot bump that moved the
// `tauri` crate in Cargo.lock (allowed by its `^2.x` requirement) while
// `@tauri-apps/api` stayed tilde-pinned passed every check and broke only the
// tag-triggered release — v1.17.0's first run failed on all four desktop legs.
//
// This compares the two *lockfiles*, i.e. the versions that actually get built,
// which is what the Tauri CLI compares too.

import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));

function lockedCrateVersion(name) {
  const lock = readFileSync(resolve(here, "../../../Cargo.lock"), "utf8");
  const versions = [...lock.matchAll(/\[\[package\]\]\r?\nname = "([^"]+)"\r?\nversion = "([^"]+)"/g)]
    .filter((m) => m[1] === name)
    .map((m) => m[2]);
  // Two copies of the crate would mean two Tauri runtimes; neither is "the" one.
  expect(versions, `exactly one locked version of the ${name} crate`).toHaveLength(1);
  return versions[0];
}

function lockedNpmVersion(name) {
  const lock = JSON.parse(readFileSync(resolve(here, "../../package-lock.json"), "utf8"));
  const pkg = lock.packages?.[`node_modules/${name}`];
  expect(pkg, `${name} in package-lock.json`).toBeDefined();
  return pkg.version;
}

const minor = (v) => v.split(".").slice(0, 2).join(".");

describe("Tauri versions", () => {
  it("the tauri crate and @tauri-apps/api are on the same minor release", () => {
    const crate = lockedCrateVersion("tauri");
    const api = lockedNpmVersion("@tauri-apps/api");
    expect(minor(api), `tauri ${crate} vs @tauri-apps/api ${api}`).toBe(minor(crate));
  });
});
