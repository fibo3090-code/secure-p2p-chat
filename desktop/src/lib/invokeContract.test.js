// The invoke-name contract: every key `bridge.js` sends must match a parameter
// the Rust command declares.
//
// A mismatch is a *silent* no-op in Tauri 2 — no throw, no log, no type error —
// which is how "messages send but don't arrive" and "fingerprint verify does
// nothing" both happened. The Rust bridge tests cover this by driving the real
// handlers, but they cannot be compiled on Windows, so this is the platform-
// independent half.

import { describe, it, expect } from "vitest";
import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import {
    countInvokeSites,
    parseInvokeCalls,
    parseRustCommands,
    contractProblems,
} from "./invokeContract.js";

const here = dirname(fileURLToPath(import.meta.url));
const bridgeSource = readFileSync(resolve(here, "bridge.js"), "utf8");
const commandsDir = resolve(here, "../../src-tauri/src/commands");
const rustSource = readdirSync(commandsDir)
    .filter((f) => f.endsWith(".rs"))
    .map((f) => readFileSync(resolve(commandsDir, f), "utf8"))
    .join("\n");

describe("parseInvokeCalls", () => {
    it("reads a call with no arguments", () => {
        expect(parseInvokeCalls('invoke("auth_status")')).toEqual([
            { command: "auth_status", keys: [] },
        ]);
    });

    it("reads shorthand and explicit keys", () => {
        expect(parseInvokeCalls('invoke("mark_read", { id })')).toEqual([
            { command: "mark_read", keys: ["id"] },
        ]);
        // `new` is a reserved word, so this one has to be written out.
        expect(
            parseInvokeCalls('invoke("change_password", { current, new: next })'),
        ).toEqual([{ command: "change_password", keys: ["current", "new"] }]);
    });
});

describe("parseRustCommands", () => {
    it("reads parameter names and drops framework-injected ones", () => {
        const src = `
#[tauri::command]
pub(crate) async fn set_presence(
    focused: bool,
    chat: Option<String>,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {`;
        expect(parseRustCommands(src)).toEqual([
            { command: "set_presence", params: ["focused", "chat"] },
        ]);
    });

    it("is not confused by generics containing commas", () => {
        const src = `
#[tauri::command]
pub(crate) async fn open_file<R: tauri::Runtime>(
    id: String,
    msg: String,
    window: tauri::WebviewWindow<R>,
    state: tauri::State<'_, Bridge>,
) -> Result<OpenOutcome, String> {`;
        expect(parseRustCommands(src)).toEqual([
            { command: "open_file", params: ["id", "msg"] },
        ]);
    });
});

describe("contractProblems", () => {
    it("passes when the names line up", () => {
        expect(
            contractProblems(
                [{ command: "mark_read", keys: ["id"] }],
                [{ command: "mark_read", params: ["id"] }],
            ),
        ).toEqual([]);
    });

    it("catches the rename that silently does nothing", () => {
        // This is the actual historical bug: JS kept sending `chat_id` after the
        // Rust parameter became `id`.
        const problems = contractProblems(
            [{ command: "mark_read", keys: ["chat_id"] }],
            [{ command: "mark_read", params: ["id"] }],
        );
        expect(problems.join("\n")).toMatch(/sends "chat_id"/);
        expect(problems.join("\n")).toMatch(/never sends "id"/);
    });

    it("catches a command that no longer exists", () => {
        const problems = contractProblems(
            [{ command: "removed_command", keys: [] }],
            [{ command: "mark_read", params: ["id"] }],
        );
        expect(problems.join("\n")).toMatch(/no #\[tauri::command\]/);
    });
});

describe("the scanner knows code from text", () => {
    it("does not see a call site inside a string or a comment", () => {
        const source = `
            // invoke("commented_out")
            /* invoke("also_commented") */
            const label = "invoke(not_a_call)";
            api.invoke("real_command", { id });
        `;
        expect(countInvokeSites(source)).toBe(1);
        expect(parseInvokeCalls(source)).toEqual([
            { command: "real_command", keys: ["id"] },
        ]);
    });

    it("does not let a quote inside a regex literal swallow the file", () => {
        // The failure this pins: a naive string-stripper treats the `"` in the
        // character class as opening a string, and everything up to the next
        // quote — including a real call site — disappears into it. The result
        // is self-consistent, so every count derived from it agrees with every
        // other and nothing looks wrong.
        const source = `
            const quoted = /["']/g;
            invoke("survives_the_regex", { id });
        `;
        expect(countInvokeSites(source)).toBe(1);
        expect(parseInvokeCalls(source)).toEqual([
            { command: "survives_the_regex", keys: ["id"] },
        ]);
    });

    it("reads a payload containing a nested object", () => {
        // `[^}]` stopped at the first `}`, so this call did not match at all —
        // and an unmatched call is an unchecked call.
        const source = `invoke("party_create_channel", { id, spec: { kind, name } });`;
        expect(countInvokeSites(source)).toBe(1);
        expect(parseInvokeCalls(source)).toEqual([
            { command: "party_create_channel", keys: ["id", "spec"] },
        ]);
    });

    it("counts a call whose command name it cannot read", () => {
        // A computed command name is not something the scanner can check. The
        // point is that it is *counted*, so the equality assertion below turns
        // it into a visible failure instead of a silent omission.
        const source = `invoke(commandName, { id });`;
        expect(countInvokeSites(source)).toBe(1);
        expect(parseInvokeCalls(source)).toEqual([]);
    });
});

describe("the real bridge", () => {
    it("parses a plausible number of commands from both sides", () => {
        // A guard on the guard: if either regex stopped matching, the contract
        // check below would pass by finding nothing to compare.
        const calls = parseInvokeCalls(bridgeSource);
        const commands = parseRustCommands(rustSource);
        expect(calls.length).toBeGreaterThan(40);
        expect(commands.length).toBeGreaterThan(40);
    });

    it("reads every invoke call site in the bridge, not most of them", () => {
        // `toBeGreaterThan(40)` was the only floor, and it is satisfied by
        // skipping a quarter of the file. A call the scanner cannot parse is
        // simply not checked — which for a test whose whole subject is silent
        // no-ops is the one way it must not fail. Equality makes an
        // unparseable call site fail loudly and name itself.
        const parsed = parseInvokeCalls(bridgeSource).length;
        const sites = countInvokeSites(bridgeSource);
        expect(parsed).toBe(sites);
    });

    it("sends exactly the arguments every Rust command declares", () => {
        const problems = contractProblems(
            parseInvokeCalls(bridgeSource),
            parseRustCommands(rustSource),
        );
        expect(problems).toEqual([]);
    });
});
