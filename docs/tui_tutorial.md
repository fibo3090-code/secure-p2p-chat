# TUI Tutorial for Encrypted P2P Messenger

## 1. Introduction

This tutorial provides a comprehensive guide to using the Terminal User Interface (TUI) for the `chat-p2p` application. The TUI allows you to interact with the secure peer-to-peer chat system directly from your terminal, offering a lightweight and efficient way to communicate without a graphical window manager.

## 2. Installation & Prerequisites

To use the TUI, you need to build the application with the TUI features.

### Prerequisites

- **Rust Toolchain**: Install via [rustup](https://rustup.rs/)
- **Terminal Emulator**: Any modern terminal (Alacritty, Windows Terminal, iTerm2, GNOME Terminal)

### Build Instructions

1. Clone the repository:

   ```bash
   git clone https://github.com/fibo3090-code/secure-p2p-chat.git
   cd secure-p2p-chat
   ```

2. Build the application in release mode:

   ```bash
   cargo build --release
   ```

3. The executable will be located at:
   - Linux/macOS: `./target/release/encodeur_rsa_rust`
   - Windows: `.\target\release\encodeur_rsa_rust.exe`

## 3. Launching the TUI

To start the application in TUI mode, use the `--tui` flag.

```bash
# Basic usage (GUI is default, so pass --tui explicitly)
./target/release/encodeur_rsa_rust --tui
```

### Command Line Arguments

- `--tui`: Force launch in Terminal User Interface mode.
- `--gui`: Force launch in Graphical User Interface mode (default).
- `--host`: Start in host mode immediately.
- `--connect <IP:PORT>`: Connect to a specific peer immediately.
- `--port <PORT>`: Specify listening port (default: 12345).

**Example:**

```bash
./target/release/encodeur_rsa_rust --tui --port 9000
```

## 4. Navigation & Controls

The TUI is designed to be fully keyboard-driven.

### Keybindings

| Key | Description |
| :--- | :--- |
| **Navigation** | |
| `↑` (Up Arrow) | Select previous chat in the sidebar |
| `↓` (Down Arrow) | Select next chat in the sidebar |
| `Page Up` | Scroll chat history up |
| `Page Down` | Scroll chat history down |
| `Tab` | Cycle focus: chats -> messages -> input |
| **Messaging** | |
| `Enter` | Send the message currently in the input field |
| `Ctrl+J` | Insert newline in the input field |
| `Backspace` | Delete the last character |
| `Esc` | Move focus back to chat list |
| **Commands** | |
| `:` | Open command palette |
| `Enter` (in command mode) | Execute command |
| **Examples** | `:host 9000`, `:connect 192.168.1.10:12345`, `:rename Team Chat`, `:disconnect`, `:help`, `:quit` |
| **System** | |
| `q` | Quit (when input box is not focused) |

### Interface Layout

The screen is divided into three main sections:

1. **Sidebar (Left)**: Displays your list of active chats and contacts.
2. **Chat Area (Right)**: Shows the message history for the selected conversation.
3. **Input/Command Bar (Bottom)**: Message input and command mode (`:`).
4. **Status Line**: Focus/mode/session summary and quick hints.

## 5. Using the TUI

### 1. Managing Chats

- Use the **Up/Down arrows** to cycle through your available chats.
- The currently selected chat will be highlighted in the sidebar.
- The message view automatically updates to show the history of the selected chat.

### 2. Sending Messages

- Type your message using your keyboard. You will see characters appear in the bottom input bar.
- Press **Enter** to send.
- Press **Ctrl+J** to insert a newline without sending.
- Your message will appear in the chat history immediately.

### 3. Exiting

- Press `q` at any time to close the application cleanly.

## 6. Troubleshooting

### "TUI looks broken or characters are misaligned"

- Ensure your terminal uses a **Monospace Font** (e.g., Fira Code, JetBrains Mono, Consolas).
- Provides adequate window size (at least 80x24 characters).

### "Key presses aren't registering"

- Verify that no other application is capturing global shortcuts.
- On some systems, `Page Up`/`Page Down` might require holding `Shift` or `Fn` depending on your keyboard layout.

### "I can't see the cursor"

- The TUI hides the system cursor by default for a cleaner look. Your typing position is indicated by the text in the input field.
