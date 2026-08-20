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

describe("the real bridge", () => {
    it("parses a plausible number of commands from both sides", () => {
        // A guard on the guard: if either regex stopped matching, the contract
        // check below would pass by finding nothing to compare.
        const calls = parseInvokeCalls(bridgeSource);
        const commands = parseRustCommands(rustSource);
        expect(calls.length).toBeGreaterThan(40);
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
