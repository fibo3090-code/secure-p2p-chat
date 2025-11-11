# Contributing to Encrypted P2P Messenger

First off, thank you for considering contributing to the Encrypted P2P Messenger! It's people like you that make this software better for everyone.

## Code of Conduct

This project and everyone participating in it is governed by our Code of Conduct. By participating, you are expected to uphold this code.

## How Can I Contribute?

### Reporting Bugs

This section guides you through submitting a bug report. Following these guidelines helps maintainers and the community understand your report, reproduce the behavior, and find related reports.

**Before Submitting A Bug Report**

- Check the documentation for tips on troubleshooting
- Perform a cursory search to see if the problem has already been reported

**How Do I Submit A (Good) Bug Report?**

Bugs are tracked as GitHub issues. Create an issue and provide the following information:

- **Use a clear and descriptive title** for the issue to identify the problem.
- **Describe the exact steps which reproduce the problem** in as many details as possible.
- **Provide specific examples to demonstrate the steps**.
- **Describe the behavior you observed after following the steps** and point out what exactly is the problem with that behavior.
- **Explain which behavior you expected to see instead and why.**
- **Include screenshots and animated GIFs** if possible.
- **Include details about your configuration and environment**

### Suggesting Enhancements

This section guides you through submitting an enhancement suggestion, including completely new features and minor improvements to existing functionality.

**Before Submitting An Enhancement Suggestion**

- Check if the enhancement has already been suggested
- Determine which repository the enhancement should be suggested in

**How Do I Submit A (Good) Enhancement Suggestion?**

Enhancement suggestions are tracked as GitHub issues. Create an issue and provide the following information:

- **Use a clear and descriptive title** for the issue to identify the suggestion.
- **Provide a step-by-step description of the suggested enhancement** in as many details as possible.
- **Provide specific examples to demonstrate the steps**.
- **Describe the current behavior and explain which behavior you expected to see instead** and why.
- **Include screenshots and animated GIFs** if possible.
- **Explain why this enhancement would be useful** to most users.

### Pull Requests

The process described here has several goals:

- Maintain the project's quality
- Fix problems that are important to users
- Engage the community in working toward the best possible software
- Enable a sustainable system for the project's maintainers to review contributions

**Please follow these steps to have your contribution considered by the maintainers:**

1. Follow all instructions in the template
2. Follow the [styleguides](#styleguides)
3. After you submit your pull request, verify that all status checks are passing

While the prerequisites above must be satisfied prior to having your pull request reviewed, the reviewer(s) may ask you to complete additional design work, tests, or other changes before your pull request can be ultimately accepted.

## Styleguides

### Git Commit Messages

- Use the present tense ("Add feature" not "Added feature")
- Use the imperative mood ("Move cursor to..." not "Moves cursor to...")
- Limit the first line to 72 characters or less
- Reference issues and pull requests liberally after the first line

### Rust Styleguide

- All Rust code must be formatted with `cargo fmt`
- All Rust code must pass `cargo clippy` with no warnings
- Write documentation for public APIs
- Follow the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)

### Documentation Styleguide

- Use [Markdown](https://daringfireball.net/projects/markdown) for documentation
- Reference functions and variables with backticks
- Follow the existing documentation style in the project

## Additional Notes

### Issue and Pull Request Labels

This section lists the labels we use to help us track and manage issues and pull requests.

- `bug` - Issues that are bugs
- `enhancement` - Issues that are feature requests
- `documentation` - Issues related to documentation
- `security` - Issues related to security
- `good first issue` - Good for newcomers
- `help wanted` - Extra attention is needed

## Getting Started

1. Fork the repository on GitHub
2. Clone your fork locally
3. Create a branch for your changes
4. Make your changes
5. Add tests if applicable
6. Run the test suite to ensure everything works
7. Commit your changes
8. Push to your fork
9. Create a pull request

## Development Setup

1. Install Rust (latest stable version)
2. Clone the repository
3. Run `cargo build` to build the project
4. Run `cargo test` to run tests

## Testing

- All code changes should be accompanied by tests
- Run `cargo test` to run all tests
- Run `cargo clippy` to check for code quality issues
- Run `cargo fmt -- --check` to check code formatting

Thank you for reading and contributing!