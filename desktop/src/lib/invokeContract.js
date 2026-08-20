// Cross-check the invoke keys `bridge.js` sends against the parameter names the
// Rust `#[tauri::command]` handlers declare.
//
// ## Why this exists
//
// Tauri 2 binds invoke arguments by exact name. A mismatch does not throw, does
// not log, and does not fail a type check — the command runs with the parameter
// missing or defaulted, so the call is a **silent no-op**. That footgun has cost
// this project real bugs already: "messages send but don't arrive" and
// "fingerprint verify does nothing" were both a JS key that no longer matched a
// Rust parameter.
//
// The Rust bridge tests catch it by driving the real handlers over mock IPC, but
// they cannot run on Windows: a Rust test-harness executable linking tauri aborts
// at startup there (`STATUS_ENTRYPOINT_NOT_FOUND` — it lacks the manifest
// `tauri-build` embeds into the real app binary). So on Windows that class of
// regression had no coverage at all.
//
// This check needs neither tauri nor a webview: it reads both sides as text and
// compares the names. It therefore runs everywhere `npm test` runs, which is now
// all three platforms — and it is strictly *more* direct than the IPC tests for
// this specific question, because it compares the two declarations rather than
// inferring agreement from behaviour.
//
// It does not replace the Rust tests. Those check that a command *works*; this
// checks that it can be *reached*.

/// Parameters every command receives from the framework rather than from JS.
/// A caller never sends these, so they must not be treated as missing keys.
const INJECTED_PARAMS = new Set(["state", "window", "app", "webview", "handle"]);

/// Extract `{ command, keys }` for every `invoke(...)` call in `bridge.js`.
///
/// Handles the three shapes the file actually uses:
///   invoke("auth_status")
///   invoke("mark_read", { id })
///   invoke("change_password", { current, new: next })
export function parseInvokeCalls(source) {
    const calls = [];
    const re = /invoke\(\s*"([a-z_0-9]+)"\s*(?:,\s*(\{[^}]*\}))?\s*\)/g;
    let m;
    while ((m = re.exec(source)) !== null) {
        const [, command, argObject] = m;
        const keys = [];
        if (argObject) {
            // Split the object literal on top-level commas. The bridge never
            // nests an object inside an invoke payload, so this is sufficient
            // and stays readable.
            for (const part of argObject.slice(1, -1).split(",")) {
                const trimmed = part.trim();
                if (!trimmed) continue;
                // `key: value` → key; shorthand `key` → key.
                const key = trimmed.split(":")[0].trim();
                if (key) keys.push(key);
            }
        }
        calls.push({ command, keys });
    }
    return calls;
}

/// Extract `{ command, params }` for every `#[tauri::command]` in a Rust source.
export function parseRustCommands(source) {
    const commands = [];
    // The `(?:<[^>]*>)?` is load-bearing: commands that open a native dialog are
    // generic over the runtime (`fn open_file<R: tauri::Runtime>(…)`), and a
    // pattern that demanded `(` straight after the name skipped every one of
    // them — which would have made this check quietly pass by not looking at the
    // six commands most likely to be edited.
    const re =
        /#\[tauri::command\][\s\S]*?fn\s+([a-z_0-9]+)\s*(?:<[^>]*>)?\s*(\([\s\S]*?\))\s*->/g;
    let m;
    while ((m = re.exec(source)) !== null) {
        const [, command, paramBlock] = m;
        const params = [];
        // Strip the outer parens, then take each parameter's name — the text
        // before the first colon at depth zero.
        const inner = paramBlock.slice(1, -1);
        let depth = 0;
        let current = "";
        const parts = [];
        for (const ch of inner) {
            if (ch === "<" || ch === "(" || ch === "[") depth++;
            else if (ch === ">" || ch === ")" || ch === "]") depth--;
            if (ch === "," && depth === 0) {
                parts.push(current);
                current = "";
            } else {
                current += ch;
            }
        }
        if (current.trim()) parts.push(current);

        for (const part of parts) {
            const name = part.trim().split(":")[0].trim();
            if (!name) continue;
            if (INJECTED_PARAMS.has(name)) continue;
            params.push(name);
        }
        commands.push({ command, params });
    }
    return commands;
}

/// Compare the two sides, returning a list of human-readable problems.
///
/// Empty means every invoke call names a command that exists and passes exactly
/// the keys that command declares.
export function contractProblems(invokeCalls, rustCommands) {
    const problems = [];
    const byName = new Map(rustCommands.map((c) => [c.command, c]));

    for (const call of invokeCalls) {
        const rust = byName.get(call.command);
        if (!rust) {
            problems.push(
                `invoke("${call.command}") has no #[tauri::command] with that name`,
            );
            continue;
        }
        const declared = new Set(rust.params);
        const sent = new Set(call.keys);

        for (const key of sent) {
            if (!declared.has(key)) {
                problems.push(
                    `invoke("${call.command}") sends "${key}", which the Rust handler does not declare ` +
                        `(it takes: ${rust.params.join(", ") || "no arguments"}). ` +
                        `Tauri binds by exact name, so this call silently does nothing.`,
                );
            }
        }
        for (const param of declared) {
            if (!sent.has(param)) {
                problems.push(
                    `invoke("${call.command}") never sends "${param}", which the Rust handler requires ` +
                        `(it takes: ${rust.params.join(", ")}).`,
                );
            }
        }
    }
    return problems;
}
