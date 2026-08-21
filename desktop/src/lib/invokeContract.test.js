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
    parseInvokeCalls,
    scanInvokeCalls,
    countInvokeSites,
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

    // The old parser used `[^}]*` for the argument object, so a nested object
    // ended the match at the *inner* brace and the call did not match at all —
    // it was skipped, silently, rather than flagged.
    it("reads a nested argument object", () => {
        expect(
            parseInvokeCalls('invoke("update_settings", { settings: { a, b }, id })'),
        ).toEqual([{ command: "update_settings", keys: ["settings", "id"] }]);
    });

    it("reads an argument object containing a brace in a string", () => {
        expect(parseInvokeCalls('invoke("open_url", { url: "a}b" })')).toEqual([
            { command: "open_url", keys: ["url"] },
        ]);
    });

    it("ignores a commented-out call", () => {
        const source = [
            '// invoke("removed_command", { id })',
            '/* invoke("also_removed") */',
            'invoke("mark_read", { id })',
        ].join("\n");
        expect(countInvokeSites(source)).toBe(1);
        expect(parseInvokeCalls(source)).toEqual([
            { command: "mark_read", keys: ["id"] },
        ]);
    });

    // A call this parser cannot read is the dangerous case: it used to vanish
    // from the subject set without a word. Now it is reported, and the real
    // bridge asserts there are none.
    it("reports a computed command name instead of skipping it", () => {
        const { calls, unparsed } = scanInvokeCalls("invoke(name, { id })");
        expect(calls).toEqual([]);
        expect(unparsed).toHaveLength(1);
        expect(unparsed[0].reason).toMatch(/not a string literal/);
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

describe("the real bridge", () => {
    it("parses every invoke call site, with none skipped", () => {
        // A guard on the guard, and it is an equality rather than a floor. A
        // floor of "more than 40" was satisfied by a parser that silently
        // skipped the calls it could not read — which were, by construction, the
        // unusual ones most likely to be wrong. If a call site cannot be parsed,
        // that is the failure; it must never be an omission.
        const { calls, unparsed } = scanInvokeCalls(bridgeSource);
        expect(
            unparsed.map((u) => `${u.reason}: ${u.snippet}`),
        ).toEqual([]);
        expect(calls.length).toBe(countInvokeSites(bridgeSource));
        expect(calls.length).toBeGreaterThan(40);

        const commands = parseRustCommands(rustSource);
        expect(commands.length).toBeGreaterThan(40);
    });

    it("sends exactly the arguments every Rust command declares", () => {
        const problems = contractProblems(
            parseInvokeCalls(bridgeSource),
            parseRustCommands(rustSource),
        );
        expect(problems).toEqual([]);
    });
});
