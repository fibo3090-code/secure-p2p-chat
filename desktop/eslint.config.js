// ESLint for the frontend.
//
// Added because a real bug got through without it. A patch applied twice left
// three duplicated JSX props in the change-password dialog; SonarCloud caught it,
// but only in CI, after a push. `react/jsx-no-duplicate-props` is a default rule
// and would have flagged it on save.
//
// The rule set is deliberately narrow. A linter that reports 400 style opinions
// on its first run gets switched off, and then it is not there for the one
// finding that mattered. What is enabled here is the set that catches *bugs* —
// things that are silently wrong at runtime rather than merely unfashionable.
// Formatting is not policed at all; there is no prettier here and this is not a
// backdoor for one.

import js from "@eslint/js";
import globals from "globals";
import react from "eslint-plugin-react";
import reactHooks from "eslint-plugin-react-hooks";

export default [
    {
        // Build output and dependencies are not ours to lint.
        ignores: ["dist/**", "node_modules/**", "src-tauri/**"],
    },
    js.configs.recommended,
    {
        files: ["**/*.{js,jsx}"],
        languageOptions: {
            ecmaVersion: 2023,
            sourceType: "module",
            parserOptions: {
                ecmaFeatures: { jsx: true },
            },
            globals: {
                ...globals.browser,
                ...globals.node,
            },
        },
        settings: { react: { version: "detect" } },
        plugins: { react, "react-hooks": reactHooks },
        rules: {
            // ── The reason this file exists ──────────────────────────────────
            // A duplicate prop is not a style question: the last one wins and
            // the earlier one is silently discarded, so the code says one thing
            // and does another.
            "react/jsx-no-duplicate-props": "error",

            // Without this, `no-unused-vars` cannot see that a binding is used
            // inside JSX and reports every imported component as dead. It is the
            // rule that makes the whole config usable, and leaving it out
            // produced 92 false errors on the first run.
            "react/jsx-uses-vars": "error",

            // ── Bugs, not opinions ──────────────────────────────────────────
            // `useEffect` with a stale closure is this app's most likely
            // rendering bug, and the poll loop makes stale reads easy to write.
            "react-hooks/rules-of-hooks": "error",
            "react-hooks/exhaustive-deps": "warn",

            // JSX that references an undefined component renders nothing and
            // says nothing.
            "react/jsx-no-undef": "error",
            "react/jsx-key": "error",
            "react/no-children-prop": "error",
            "react/jsx-no-target-blank": "error",

            // `react/prop-types` wants PropTypes declarations on every
            // component. This codebase deliberately does not use them, so the
            // rule would report every file and teach people to ignore output.
            "react/prop-types": "off",
            // JSX does not need React in scope under the modern transform.
            "react/react-in-jsx-scope": "off",

            // ── From the recommended set, tuned ─────────────────────────────
            // Unused *arguments* are often deliberate (a callback signature you
            // must accept but do not use), so only flag unused variables, and
            // allow the conventional leading underscore to opt out.
            "no-unused-vars": [
                "error",
                {
                    args: "none",
                    varsIgnorePattern: "^_",
                    caughtErrors: "none",
                },
            ],
            // An empty catch is how errors get swallowed. This codebase does it
            // intentionally in places and comments why, so warn rather than
            // error — but do not let it be invisible.
            "no-empty": ["warn", { allowEmptyCatch: true }],
        },
    },
    {
        // Test files get the test globals and a lighter touch: a deliberately
        // malformed fixture is the point of a test, not a mistake in it.
        files: ["**/*.test.{js,jsx}", "src/test/**"],
        languageOptions: {
            globals: { ...globals.node, ...globals.browser },
        },
        rules: {
            "no-unused-vars": "off",
        },
    },
];
