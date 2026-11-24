// ESLint for the frontend. Deliberately narrow: rules that catch *bugs* (silently
// wrong at runtime), not style — a linter that reports 400 opinions gets switched
// off. Formatting is not policed.

import js from "@eslint/js";
import globals from "globals";
import react from "eslint-plugin-react";
import reactHooks from "eslint-plugin-react-hooks";

export default [
    {
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
            // Why this file exists: duplicated JSX props once shipped (the last
            // silently wins).
            "react/jsx-no-duplicate-props": "error",

            // Lets `no-unused-vars` see bindings used in JSX.
            "react/jsx-uses-vars": "error",

            // Stale `useEffect` closures are this app's likeliest render bug.
            "react-hooks/rules-of-hooks": "error",
            "react-hooks/exhaustive-deps": "warn",

            // An undefined component renders nothing and says nothing.
            "react/jsx-no-undef": "error",
            "react/jsx-key": "error",
            "react/no-children-prop": "error",
            "react/jsx-no-target-blank": "error",

            // No PropTypes in this codebase; no React in scope with the modern transform.
            "react/prop-types": "off",
            "react/react-in-jsx-scope": "off",

            // Unused arguments are often a required callback signature; `_x` opts out.
            "no-unused-vars": [
                "error",
                {
                    args: "none",
                    varsIgnorePattern: "^_",
                    caughtErrors: "none",
                },
            ],
            // Deliberate (commented) empty catches exist, so warn, not error.
            "no-empty": ["warn", { allowEmptyCatch: true }],
        },
    },
    {
        // Playwright code has no React, and rules-of-hooks misreads its `use`
        // fixture callback as React's `use` hook.
        files: ["smoke/**", "playwright.config.js"],
        languageOptions: { globals: { ...globals.node } },
        rules: {
            "react-hooks/rules-of-hooks": "off",
            "react-hooks/exhaustive-deps": "off",
        },
    },
    {
        // Tests: test globals, and a malformed fixture is the point, not a mistake.
        files: ["**/*.test.{js,jsx}", "src/test/**"],
        languageOptions: {
            globals: { ...globals.node, ...globals.browser },
        },
        rules: {
            "no-unused-vars": "off",
        },
    },
];
