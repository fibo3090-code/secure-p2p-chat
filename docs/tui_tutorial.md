# TUI Tutorial for chat-p2p

## 1. Introduction
This tutorial provides a comprehensive guide to using the Terminal User Interface (TUI) for the `chat-p2p` application. The TUI allows users to interact with the secure peer-to-peer chat system directly from their terminal, offering a lightweight and efficient way to communicate.

## 2. Installation
Before you can use the `chat-p2p` TUI, ensure you have the necessary prerequisites and follow the installation steps below.

### Prerequisites
- Rust programming language and Cargo package manager (install via rustup: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`)
- A compatible terminal emulator (e.g., Alacritty, GNOME Terminal, iTerm2, Windows Terminal)

### Steps
1. Clone the `chat-p2p` repository:
   ```bash
   git clone https://github.com/your-repo/chat-p2p.git
   cd chat-p2p
   ```
2. Build the application with TUI feature enabled:
   ```bash
   cargo build --release --features tui
   ```
3. The executable will be located in `target/release/chat-p2p`.

## 3. Basic Usage and Navigation
This section covers the fundamental aspects of interacting with the `chat-p2p` TUI, including navigation and common keybindings.

### Starting the TUI
To start the TUI, run the compiled executable:
```bash
./target/release/chat-p2p
```
(Note: The TUI is the default interface when running the application without specific arguments.)

### Keybindings
| Key          | Action                       | Description                               |
|--------------|------------------------------|-------------------------------------------|
| `q`          | Quit                         | Exit the application.                     |
| `Down Arrow` | Select Next Chat             | Move to the next chat in the list.        |
| `Up Arrow`   | Select Previous Chat         | Move to the previous chat in the list.    |
| `Page Down`  | Scroll Messages Down         | Scroll the message view downwards.        |
| `Page Up`    | Scroll Messages Up           | Scroll the message view upwards.          |
| `Enter`      | Send Message                 | Send the text currently in the input field. |
| Any Char     | Type                         | Enter characters into the input field.    |
| `Backspace`  | Delete Character             | Delete the last character in the input field. |

## 4. Core Commands

The `chat-p2p` TUI provides a straightforward set of interactions primarily driven by key presses.

### Sending Messages
1.  **Select a Chat:** Use the `Up Arrow` and `Down Arrow` keys to navigate and select a chat from the "Chats" panel on the left.
2.  **Type your Message:** As you type, characters will appear in the "Input" box at the bottom right.
3.  **Send Message:** Press `Enter` to send the typed message to the selected chat. The input box will clear automatically after sending.

### Navigating Chats
-   **Next Chat:** Press `Down Arrow` to move the selection to the next chat in your chat list. The selection will wrap around to the first chat if you are at the end of the list.
-   **Previous Chat:** Press `Up Arrow` to move the selection to the previous chat in your chat list. The selection will wrap around to the last chat if you are at the beginning of the list.

### Scrolling Through Messages
-   **Scroll Up:** Use `Page Up` to scroll upwards through the message history of the currently selected chat.
-   **Scroll Down:** Use `Page Down` to scroll downwards through the message history of the currently selected chat.

### Exiting the Application
-   **Quit:** Press `q` to exit the `chat-p2p` TUI application.

## 5. Advanced Features
The current Terminal User Interface (TUI) for `chat-p2p` primarily focuses on providing a streamlined experience for basic chat functionalities, including sending and receiving messages, and navigating between active chat sessions.

Advanced features such as file transfers, group chat management, or detailed identity management are handled by the underlying `ChatManager` but are not directly exposed or controllable through specific TUI commands or keybindings in this version. Future iterations may expand TUI capabilities to include more direct interaction with these advanced features.

For now, please refer to the application's core documentation for details on these features and how they might be configured or utilized outside the immediate TUI interface.

## 6. Troubleshooting
Encountering issues? This section provides solutions to common problems you might face while using the `chat-p2p` TUI.

### Common Issues
- **TUI not starting:**
  - **Solution:** Ensure you built with `--features tui` and the executable path is correct. Check for any error messages in the terminal output.
- **Keybindings not working:**
  - **Solution:** Verify your terminal emulator is compatible and not intercepting key presses.
- **Connection issues:**
  - **Solution:** Check network connectivity and ensure other peers are discoverable. Refer to the networking documentation for more details.

## 7. Usage Examples
This section provides practical scenarios to demonstrate how to use the `chat-p2p` TUI effectively.

### Example 1: Sending a direct message
1.  **Start the TUI:** Execute `./target/release/chat-p2p` in your terminal.
2.  **Select a Chat:** Use the `Up Arrow` or `Down Arrow` keys to highlight the desired chat in the "Chats" panel.
3.  **Type Your Message:** Begin typing. Your input will appear in the "Input" box at the bottom of the screen.
4.  **Send Message:** Press the `Enter` key. The message will be sent to the selected chat and appear in the messages view. The input box will then clear, ready for your next message.

### Example 2: Navigating Between Chats
1.  **View Current Chat:** Upon launching, the first available chat will be selected by default, and its messages will be displayed.
2.  **Move to Next Chat:** Press the `Down Arrow` key. The highlight in the "Chats" panel will move to the next chat, and the messages view will update to show the conversation history of the newly selected chat.
3.  **Move to Previous Chat:** Press the `Up Arrow` key. The highlight will move to the previous chat, updating the messages view accordingly. This allows you to quickly switch contexts between different conversations.
