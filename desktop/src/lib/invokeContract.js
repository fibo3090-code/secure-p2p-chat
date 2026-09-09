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

/// Neutralise everything that is text rather than code, keeping the source's
/// exact length and line structure.
///
/// The scan below is a regex over text, so it has to be told what is *code*.
/// Comments alone are not enough — `invoke("log", { msg: "invoke(x)" })` would
/// otherwise look like a second, unparseable call — and handling strings
/// without also handling regex literals is worse than handling neither: a regex
/// containing a quote, say `/["']/`, opens a string that then swallows real
/// code up to the next matching quote, and the self-consistent nonsense that
/// results passes every assertion made against it.
///
/// Comments and regex literals are blanked outright; nothing inside them is
/// ever wanted. String and template literals keep their *contents* — the first
/// argument to `invoke` is a string, and blanking it would erase the command
/// name this whole file exists to read — and lose only the structural
/// characters the scanner keys on: brackets and separators. So `"invoke(x)"`
/// stops looking like a call site and a brace inside a string stops confusing
/// the brace matcher, while `"auth_status"` still reads as `auth_status`.
///
/// Spaces rather than deletion, so offsets stay intact and a future check that
/// wants a line number gets the right one.
export function blankNonCode(source) {
    let out = "";
    let i = 0;
    // Whether a `/` here starts a regex literal or is a division sign is
    // decided by what came before it — the same rule a JS tokeniser uses.
    let prevSignificant = "";
    const blank = (text) => text.replace(/[^\n]/g, " ");

    while (i < source.length) {
        const ch = source[i];
        const next = source[i + 1];

        if (ch === "/" && next === "/") {
            const end = source.indexOf("\n", i);
            const stop = end === -1 ? source.length : end;
            out += blank(source.slice(i, stop));
            i = stop;
            continue;
        }
        if (ch === "/" && next === "*") {
            const end = source.indexOf("*/", i + 2);
            const stop = end === -1 ? source.length : end + 2;
            out += blank(source.slice(i, stop));
            i = stop;
            continue;
        }
        if (ch === '"' || ch === "'" || ch === "`") {
            let j = i + 1;
            while (j < source.length) {
                if (source[j] === "\\") {
                    j += 2;
                    continue;
                }
                if (source[j] === ch) break;
                j++;
            }
            const stop = Math.min(j + 1, source.length);
            const body = source.slice(i + 1, stop - 1);
            out += ch + body.replace(/[(){}[\],;]/g, " ") + (source[stop - 1] ?? "");
            i = stop;
            prevSignificant = ch;
            continue;
        }
        if (ch === "/" && startsRegex(prevSignificant)) {
            let j = i + 1;
            let inClass = false;
            while (j < source.length) {
                const c = source[j];
                if (c === "\\") {
                    j += 2;
                    continue;
                }
                if (c === "[") inClass = true;
                else if (c === "]") inClass = false;
                else if (c === "/" && !inClass) break;
                else if (c === "\n") break; // unterminated; treat as division
                j++;
            }
            if (source[j] === "/") {
                // Include the trailing flags.
                let k = j + 1;
                while (k < source.length && /[a-z]/.test(source[k])) k++;
                out += blank(source.slice(i, k));
                i = k;
                prevSignificant = "/";
                continue;
            }
            // Not a regex after all — fall through and treat it as an operator.
        }

        out += ch;
        if (!/\s/.test(ch)) prevSignificant = ch;
        i++;
    }
    return out;
}

/// A `/` starts a regex literal when the previous significant character cannot
/// end an expression. Deliberately conservative: over-reading a division as a
/// regex would blank real code, so anything that could be a value ends the
/// expression.
function startsRegex(prev) {
    if (prev === "") return true;
    return !/[a-zA-Z0-9_$)\]}"'`]/.test(prev);
}

/// Count the `invoke(` call sites in a source, so a call the scanner *cannot*
/// read is a loud failure rather than a silent omission.
///
/// This exists because the scanner is a regex, and a regex has an accept set
/// narrower than the language. A call it does not match simply is not checked —
/// the contract test then passes by having nothing to compare, which is the
/// failure mode a contract test must not have.
export function countInvokeSites(source) {
    const matches = blankNonCode(source).match(/\binvoke\s*\(/g);
    return matches ? matches.length : 0;
}

/// Extract `{ command, keys }` for every `invoke(...)` call in `bridge.js`.
///
/// Handles the shapes the file actually uses:
///   invoke("auth_status")
///   invoke("mark_read", { id })
///   invoke("change_password", { current, new: next })
///   invoke("party_post", { id, channel: { kind, name } })
///
/// The argument object is matched by counting braces rather than with `[^}]`.
/// The old pattern stopped at the first `}`, so a payload with a nested object
/// did not match at all and the call was skipped — silently, which for a check
/// whose entire purpose is catching silent no-ops was the wrong way to fail.
/// `countInvokeSites` is the backstop: anything this misses is still counted.
export function parseInvokeCalls(source) {
    const code = blankNonCode(source);
    const calls = [];
    const re = /\binvoke\(\s*"([a-z_0-9]+)"\s*(,\s*\{)?/g;
    let m;
    while ((m = re.exec(code)) !== null) {
        const [, command, hasArgs] = m;
        const keys = [];
        if (hasArgs) {
            const open = code.indexOf("{", m.index + m[0].length - 1);
            const close = matchingBrace(code, open);
            if (close === -1) continue;
            const inner = code.slice(open + 1, close);
            for (const part of splitTopLevel(inner)) {
                const trimmed = part.trim();
                if (!trimmed) continue;
                // `key: value` → key; shorthand `key` → key.
                const key = trimmed.split(":")[0].trim();
                if (key) keys.push(key);
            }
            re.lastIndex = close;
        }
        calls.push({ command, keys });
    }
    return calls;
}

/// Index of the `}` closing the `{` at `open`, or -1.
function matchingBrace(source, open) {
    let depth = 0;
    for (let i = open; i < source.length; i++) {
        if (source[i] === "{") depth++;
        else if (source[i] === "}") {
            depth--;
            if (depth === 0) return i;
        }
    }
    return -1;
}

/// Split an object literal's body on commas that are not inside a nested
/// brace, bracket or paren.
function splitTopLevel(inner) {
    const parts = [];
    let depth = 0;
    let current = "";
    for (const ch of inner) {
        if (ch === "{" || ch === "[" || ch === "(") depth++;
        else if (ch === "}" || ch === "]" || ch === ")") depth--;
        if (ch === "," && depth === 0) {
            parts.push(current);
            current = "";
        } else {
            current += ch;
        }
    }
    if (current.trim()) parts.push(current);
    return parts;
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
