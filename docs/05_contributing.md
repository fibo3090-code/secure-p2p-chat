# 5. Contributing

First off, thank you for considering contributing to the Encrypted P2P Messenger! It's people like you that make this software better for everyone.

This document provides guidelines for contributing to the project. Please read it carefully to ensure that the contribution process is smooth for both you and the project maintainers.

## Code of Conduct

This project and everyone participating in it is governed by our [Code of Conduct](CODE_OF_CONDUCT.md). By participating, you are expected to uphold this code.

## How Can I Contribute?

### Reporting Bugs

If you find a bug, please help us by submitting a detailed bug report.

**Before Submitting a Bug Report:**

-   Check the existing documentation for troubleshooting tips.
-   Search the GitHub issues to see if the problem has already been reported.

**How to Submit a Good Bug Report:**

Create an issue on GitHub and provide the following information:

-   **Use a clear and descriptive title.**
-   **Describe the exact steps to reproduce the problem.**
-   **Describe the behavior you observed and why it's a problem.**
-   **Explain the behavior you expected to see instead.**
-   **Include screenshots or animated GIFs** if they help to illustrate the issue.
-   **Provide details about your environment**, such as your operating system and the version of the application you are using.

### Suggesting Enhancements

If you have an idea for a new feature or an improvement to an existing one, we'd love to hear it.

**How to Submit a Good Enhancement Suggestion:**

Create an issue on GitHub and provide the following information:

-   **Use a clear and descriptive title.**
-   **Provide a detailed description of the suggested enhancement.**
-   **Explain why this enhancement would be useful** to most users.
-   **Include screenshots or mockups** if possible.

### Pull Requests

We welcome pull requests for bug fixes and new features.

**Pull Request Process:**

1.  **Create your branch from `main`** using a descriptive name (e.g., `feat/emoji-picker`, `fix/handshake-timeout`).
2.  **Make your changes**, adhering to the style guides below.
3.  **Add tests** for any new functionality.
4.  **Ensure all tests pass locally** by running `cargo test`.
5.  **Format and lint your code** by running `cargo fmt` and `cargo clippy`.
6.  **Commit your changes** using [Conventional Commits](https://www.conventionalcommits.org/).
7.  **Push your branch** to your fork and create a pull request.
8.  **Verify that all status checks are passing** on your pull request.

**PR Checklist:**

-   [ ] Linked issue (if applicable).
-   [ ] Tests added or updated.
-   [ ] `cargo fmt` has been applied.
-   [ ] `cargo clippy` has no warnings.
-   [ ] Documentation has been updated as needed (`README.md`, `DEVELOPER_GUIDE.md`, etc.).

## Style Guides

### Git Commit Messages

-   Use the present tense ("Add feature" not "Added feature").
-   Use the imperative mood ("Move cursor to..." not "Moves cursor to...").
-   Limit the first line to 72 characters or less.
-   Use Conventional Commits types: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`.

### Rust Style Guide

-   All Rust code must be formatted with `cargo fmt`.
-   All Rust code must pass `cargo clippy` with no warnings.
-   Write documentation for all public APIs.
-   Follow the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/).

### Documentation Style Guide

-   Use Markdown for all documentation.
-   Reference functions, variables, and file names with backticks.
-   Follow the existing documentation style in the project.

## Local Development

### Branching Strategy

-   The `main` branch is the primary development branch.
-   All pull requests should be made from feature branches. Keep your feature branches focused and small.

### Build and Test

-   **Build**: `cargo build`
-   **Build (Release)**: `cargo build --release`
-   **Run tests**: `cargo test`
-   **Format and Lint**: `cargo fmt && cargo clippy`

### Releases

1.  Update `CHANGELOG.md`.
2.  Bump the version in `Cargo.toml`.
3.  Create a tagged release on GitHub with detailed release notes.
