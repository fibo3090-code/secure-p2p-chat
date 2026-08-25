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

/// Keywords after which a `/` opens a regular expression rather than dividing.
/// Without them `return /["']/` reads as "identifier, then division".
const REGEX_PRECEDING_KEYWORDS = new Set([
    "return",
    "typeof",
    "instanceof",
    "in",
    "of",
    "new",
    "delete",
    "void",
    "throw",
    "case",
    "do",
    "else",
    "yield",
    "await",
]);

/// Whether a `/` appearing after `out` opens a regex literal or is a division.
///
/// JavaScript cannot be tokenised without this distinction, and the rule is
/// positional: after a *value* — an identifier, a number, `)`, `]` — a slash
/// divides; anywhere an expression may begin, it opens a regex.
function regexCanStartAfter(out) {
    const before = out.replace(/\s+$/, "");
    if (before === "") return true;
    const last = before[before.length - 1];
    if (/[)\]}]/.test(last)) return false;
    if (/[\w$]/.test(last)) {
        // An identifier or number — a divisor, unless it is one of the keywords
        // that can only be followed by the start of an expression.
        const word = before.match(/[\w$]+$/)?.[0] ?? "";
        return REGEX_PRECEDING_KEYWORDS.has(word);
    }
    return true;
}

/// Strip comments, leaving string literals and regex literals intact.
///
/// Needed so a commented-out `invoke(...)` is not counted as a call site — the
/// count is an assertion now, so a miscount is a failing test rather than a
/// quietly smaller subject set.
///
/// Regex literals are recognised rather than left to fall through, because that
/// is where this used to go wrong: a pattern containing a quote — `/["']/`, of
/// the kind a few lines below — opened what looked like a string literal, so
/// everything up to the next unrelated quote was swallowed as string contents,
/// taking any real `invoke(` site in between with it. Nothing caught that,
/// because `scanInvokeCalls` and `countInvokeSites` both read this function's
/// output: they agreed with each other about a source neither had seen whole.
function stripComments(source) {
    let out = "";
    let i = 0;
    while (i < source.length) {
        const ch = source[i];
        const next = source[i + 1];
        if (ch === "/" && next === "/") {
            while (i < source.length && source[i] !== "\n") i++;
            continue;
        }
        if (ch === "/" && next === "*") {
            i += 2;
            while (i < source.length && !(source[i] === "*" && source[i + 1] === "/")) i++;
            i += 2;
            continue;
        }
        if (ch === "/" && regexCanStartAfter(out)) {
            out += ch;
            i++;
            // A `/` inside a character class is literal, so `[/]` does not end
            // the pattern and must not be read as if it did.
            let inClass = false;
            while (i < source.length) {
                const c = source[i];
                out += c;
                i++;
                if (c === "\\") {
                    out += source[i] ?? "";
                    i++;
                    continue;
                }
                if (c === "\n") break; // unterminated — not a regex after all
                if (c === "[") inClass = true;
                else if (c === "]") inClass = false;
                else if (c === "/" && !inClass) break;
            }
            while (i < source.length && /[a-z]/.test(source[i])) {
                out += source[i]; // flags
                i++;
            }
            continue;
        }
        if (ch === '"' || ch === "'" || ch === "`") {
            const quote = ch;
            out += ch;
            i++;
            while (i < source.length) {
                out += source[i];
                if (source[i] === "\\") {
                    out += source[i + 1] ?? "";
                    i += 2;
                    continue;
                }
                if (source[i] === quote) {
                    i++;
                    break;
                }
                i++;
            }
            continue;
        }
        out += ch;
        i++;
    }
    return out;
}

const OPENERS = { "{": "}", "[": "]", "(": ")" };
const CLOSERS = new Set(["}", "]", ")"]);

/// Read the balanced region starting at `start` (which must be an opener),
/// returning `{ text, end }` where `end` is the index just past the closer.
/// Returns `null` if it never closes. String literals are skipped whole.
function readBalanced(source, start) {
    const stack = [OPENERS[source[start]]];
    let i = start + 1;
    while (i < source.length) {
        const ch = source[i];
        if (ch === '"' || ch === "'" || ch === "`") {
            const quote = ch;
            i++;
            while (i < source.length) {
                if (source[i] === "\\") {
                    i += 2;
                    continue;
                }
                if (source[i] === quote) break;
                i++;
            }
            i++;
            continue;
        }
        if (OPENERS[ch]) {
            stack.push(OPENERS[ch]);
        } else if (CLOSERS.has(ch)) {
            if (stack[stack.length - 1] !== ch) return null;
            stack.pop();
            if (stack.length === 0) {
                return { text: source.slice(start, i + 1), end: i + 1 };
            }
        }
        i++;
    }
    return null;
}

/// Split an object literal's body on commas at depth zero, skipping strings.
function topLevelKeys(objectText) {
    const inner = objectText.slice(1, -1);
    const keys = [];
    let depth = 0;
    let current = "";
    let i = 0;
    while (i < inner.length) {
        const ch = inner[i];
        if (ch === '"' || ch === "'" || ch === "`") {
            const quote = ch;
            current += ch;
            i++;
            while (i < inner.length) {
                current += inner[i];
                if (inner[i] === "\\") {
                    current += inner[i + 1] ?? "";
                    i += 2;
                    continue;
                }
                if (inner[i] === quote) {
                    i++;
                    break;
                }
                i++;
            }
            continue;
        }
        if (OPENERS[ch]) depth++;
        else if (CLOSERS.has(ch)) depth--;

        if (ch === "," && depth === 0) {
            keys.push(current);
            current = "";
        } else {
            current += ch;
        }
        i++;
    }
    if (current.trim()) keys.push(current);

    return keys
        // `key: value` -> key; shorthand `key` -> key. The colon split is safe
        // here because everything nested has been kept inside `current`.
        .map((part) => part.trim().split(":")[0].trim())
        .filter(Boolean);
}

/// Scan `source` for every `invoke(...)` call site.
///
/// Returns `{ calls, unparsed }`. `calls` is `{ command, keys }` per call;
/// `unparsed` names every site this parser could not read, with the reason.
///
/// The distinction is the point. This used to be one regex with `[^}]*` for the
/// argument object, which meant a call with a **nested** object, or a computed
/// command name, simply did not match — and a call that does not match is not
/// checked, silently. The check would have kept passing while the calls most
/// likely to be wrong went unexamined. The old test's floor of "more than 40"
/// could not catch that either; the count is now asserted exactly, and anything
/// unreadable is reported rather than skipped.
///
/// Handles:
///   invoke("auth_status")
///   invoke("mark_read", { id })
///   invoke("change_password", { current, new: next })
///   invoke("update_settings", { settings: { theme, dir } })   <- nested
export function scanInvokeCalls(source) {
    const code = stripComments(source);
    const calls = [];
    const unparsed = [];

    const site = /\binvoke\s*\(/g;
    let m;
    while ((m = site.exec(code)) !== null) {
        const openParen = m.index + m[0].length - 1;
        const snippet = code.slice(m.index, m.index + 80).split("\n")[0];

        const args = readBalanced(code, openParen);
        if (!args) {
            unparsed.push({ snippet, reason: "argument list never closes" });
            continue;
        }
        // Re-scan from past this call, so a nested `invoke(` inside it is not
        // matched twice.
        site.lastIndex = args.end;

        const inner = args.text.slice(1, -1).trim();
        if (!inner.startsWith('"')) {
            unparsed.push({
                snippet,
                reason: "command name is not a string literal",
            });
            continue;
        }
        const closeQuote = inner.indexOf('"', 1);
        if (closeQuote === -1) {
            unparsed.push({ snippet, reason: "unterminated command name" });
            continue;
        }
        const command = inner.slice(1, closeQuote);

        const rest = inner.slice(closeQuote + 1).trim();
        if (rest === "") {
            calls.push({ command, keys: [] });
            continue;
        }
        if (!rest.startsWith(",")) {
            unparsed.push({ snippet, reason: "unexpected text after the command name" });
            continue;
        }
        const afterComma = rest.slice(1).trim();
        if (!afterComma.startsWith("{")) {
            unparsed.push({
                snippet,
                reason: "argument is not an object literal",
            });
            continue;
        }
        const obj = readBalanced(afterComma, 0);
        if (!obj || obj.end !== afterComma.length) {
            unparsed.push({ snippet, reason: "argument object is malformed" });
            continue;
        }
        calls.push({ command, keys: topLevelKeys(obj.text) });
    }

    return { calls, unparsed };
}

/// How many `invoke(` call sites the source contains, counted independently of
/// whether they could be parsed. The test asserts this equals `calls.length`.
export function countInvokeSites(source) {
    return (stripComments(source).match(/\binvoke\s*\(/g) || []).length;
}

/// The parsed calls, for callers that do not care about the diagnostics.
export function parseInvokeCalls(source) {
    return scanInvokeCalls(source).calls;
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
