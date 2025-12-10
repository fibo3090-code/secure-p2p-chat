# AGENTS.md - Development Guidelines

## Build/Test Commands
- `cargo build` - Build the project
- `cargo test` - Run all tests
- `cargo test <test_name>` - Run single test
- `cargo run -- --help` - Show CLI options
- `cargo clippy` - Lint checks
- `cargo fmt` - Format code

## Code Style Guidelines

### Imports & Structure
- Use `use crate::*` for internal imports in main.rs
- Group external imports alphabetically
- Use `anyhow::Result` for error handling
- Prefer `tokio::spawn` for async tasks

### Naming Conventions
- `PascalCase` for types, structs, enums
- `snake_case` for functions and variables
- `SCREAMING_SNAKE_CASE` for constants
- Use descriptive names (e.g., `chat_id`, `peer_fingerprint`)

### Error Handling
- Use `anyhow::Result<T>` for fallible functions
- Use `anyhow::anyhow!("message")` for custom errors
- Use `?` operator for error propagation
- Log errors with `tracing::error!`

### Types & Serialization
- Use `#[derive(Serialize, Deserialize, Debug, Clone)]` for data types
- Use `#[serde(skip)]` for non-serializable fields
- Use `Uuid` for IDs, `chrono::Utc::now()` for timestamps
- Use `Option<T>` for nullable fields

### Async & Concurrency
- Use `tokio::sync::mpsc` for channels
- Use `Arc<Mutex<T>>` for shared state
- Use `#[tokio::main]` for async main functions
- Use `tokio::task::spawn_blocking` for CPU-heavy operations

### Security
- Never log secrets or private keys
- Use `zeroize` for sensitive data cleanup
- Validate all external inputs
- Use constant-time comparisons for crypto operations