// Cross-check the invoke keys `bridge.js` sends against the parameter names the
// Rust `#[tauri::command]` handlers declare. Tauri 2 binds arguments by exact
// name and a mismatch is a silent no-op ("messages send but don't arrive").
// The Rust IPC tests catch it too, but cannot run on Windows (a test harness
// linking tauri aborts at startup there); this reads both sides as text, so it
// runs wherever `npm test` does. It checks that a command can be *reached*;
// the Rust tests check that it *works*.

/// Parameters the framework injects; a caller never sends these.
const INJECTED_PARAMS = new Set(["state", "window", "app", "webview", "handle"]);

/// Neutralise everything that is text rather than code, keeping the source's
/// exact length and line structure (so offsets stay valid).
///
/// Comments and regex literals are blanked outright (a regex containing a
/// quote would otherwise open a string that swallows real code). String and
/// template literals keep their contents — the command name is a string — and
/// lose only brackets and separators, so `"invoke(x)"` stops looking like a
/// call site.
export function blankNonCode(source) {
    let out = "";
    let i = 0;
    // A `/` starts a regex or is division depending on what came before it.
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
/// end an expression. Conservative: misreading a division would blank code.
function startsRegex(prev) {
    if (prev === "") return true;
    return !/[a-zA-Z0-9_$)\]}"'`]/.test(prev);
}

/// Count the `invoke(` call sites, so a call the scanner *cannot* read fails
/// loudly instead of passing by having nothing to compare.
export function countInvokeSites(source) {
    const matches = blankNonCode(source).match(/\binvoke\s*\(/g);
    return matches ? matches.length : 0;
}

/// Extract `{ command, keys }` for every `invoke(...)` call in `bridge.js`,
/// including nested payload objects (braces are counted, not `[^}]`).
/// `countInvokeSites` is the backstop for anything this misses.
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

/// Split a list on commas that are not inside a nested bracket pair.
function splitTopLevel(inner, opens = "{[(", closes = "}])") {
    const parts = [];
    let depth = 0;
    let current = "";
    for (const ch of inner) {
        if (opens.includes(ch)) depth++;
        else if (closes.includes(ch)) depth--;
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
    // `(?:<[^>]*>)?`: dialog commands are generic (`fn open_file<R: Runtime>(…)`).
    const re =
        /#\[tauri::command\][\s\S]*?fn\s+([a-z_0-9]+)\s*(?:<[^>]*>)?\s*(\([\s\S]*?\))\s*->/g;
    let m;
    while ((m = re.exec(source)) !== null) {
        const [, command, paramBlock] = m;
        const params = [];
        // Each parameter's name is the text before its first colon.
        for (const part of splitTopLevel(paramBlock.slice(1, -1), "<([", ">)]")) {
            const name = part.trim().split(":")[0].trim();
            if (!name) continue;
            if (INJECTED_PARAMS.has(name)) continue;
            params.push(name);
        }
        commands.push({ command, params });
    }
    return commands;
}

/// Compare the two sides: human-readable problems, empty when every call names
/// an existing command and passes exactly its declared keys.
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
