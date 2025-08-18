# Watcher

Watcher is a modern rewrite and updated version of my previous application, "circuit-watcher", built with the Tauri framework.

### Installation/Building

You can download the windows installer from the [releases page](https://github.com/TacticalDeux/watcher/releases)

To build this project you'll need to have Node.js and Rust installed on your system.

1.  **Clone the repository:**
    ```bash
    git clone https://github.com/TacticalDeux/watcher.git
    cd watcher
    ```

2.  **Install frontend dependencies:**
    ```bash
    npm install
    ```

3.  **Install Rust toolchain (if not already installed):**
    ```bash
    rustup install stable
    rustup update
    ```

4.  **Add the Tauri CLI:**
    ```bash
    cargo install tauri-cli
    ```

To build the application for your platform:

```bash
npm run tauri build
```

This will generate the executable in `src-tauri/target/release/`.
And the installer in `src-tauri/target/release/bundle/(installers)`.

### Running in Development Mode

To run the application in development mode:

```bash
npm run tauri dev
```

Using [Irelia](https://github.com/AlsoSylv/Irelia) for LCU/Websocket operations

---

This is not an official Riot Games product. It's not affiliated with or endorsed by Riot Games Inc.
