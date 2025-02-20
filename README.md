# sgh 🚀

`sgh` is a **TUI (Text-based User Interface) tool** that parses and aggregates multiple SSH configuration files, allowing you to **search** and **connect** to your SSH hosts effortlessly. With a built-in search bar, simple keybindings, and customizable command templates (powered by Handlebars), you can quickly find the right host and jump into your SSH session.  

## Features ✨

- **Multiple Config Files**: By default, it reads both `/etc/ssh/ssh_config` and `~/.ssh/config`, merging their contents seamlessly.
- **Fuzzy Search**: Type in the search bar to quickly filter hosts by name, alias, or destination.
- **SSH Command Templates**: Use Handlebars templates (e.g. `ssh "{{{name}}}"`) to define how you connect to a host.
- **Session Hooks**: Optional `--on-session-start-template` and `--on-session-end-template` let you run extra commands before and after SSH.
- **LocalForward & ProxyCommand**: View local forwarding and proxy details for your selected host.
- **TUI Navigation**:  
  - <kbd>↑</kbd>/<kbd>↓</kbd> to move selection  
  - <kbd>Enter</kbd> to connect  
  - <kbd>Esc</kbd> or <kbd>Ctrl+C</kbd> to quit  

## Installation 📦

### 1. With Cargo
```bash
git clone https://example.com/your/sgh.git
cd sgh
cargo install --path .
```

Or build locally:
```bash
cargo build --release
./target/release/sgh --help
```

### 2. Via Nix Flakes
If you have Nix and Flakes enabled:

```bash
# Build for your system, e.g. x86_64-linux
nix build .#sgh
./result/bin/sgh --help


# Or simply run
nix run .#sgh -- --help
```
## Usage 🛠️
```bash
sgh [OPTIONS]
```

Key CLI Options:

* -c, --config <PATH>...: Provide one or more custom SSH config files (defaults to /etc/ssh/ssh_config and ~/.ssh/config).
* --show-proxy-command: Show ProxyCommand details in the UI table.
* -s, --search <FILTER>: Start sgh with an initial search filter.
* --sort: Sort hosts by name (--sort=false to disable).
* -t, --template <TMPL>: A Handlebars template for your SSH command (default: ssh "{{{name}}}").
* --on-session-start-template <TMPL>: Extra command (Handlebars) to run before starting an SSH session.
* --on-session-end-template <TMPL>: Extra command (Handlebars) to run after ending an SSH session.
* -e, --exit: Exit sgh immediately after the SSH session ends.
Example:

```bash
# Fuzzy-search for "web" as soon as it starts
sgh --search web
```

## TUI Controls 🧩
* Search Bar: Type to fuzzy-filter hosts in real time.
* Arrow Keys: Navigate the host list.
* Enter: Connect to the selected host using your specified template.
* Esc or Ctrl+C: Exit sgh.
* LocalForward: Once a host is highlighted, any LocalForward rules are shown in the bottom panel.
*
## Future Ideas 📝
Tab-based UI: Switch between a search mode and a command history mode in the same TUI.
RemoteForward: Display RemoteForward rules similarly to LocalForward.
Extensive Hooks: More advanced session templates or triggers.

## License 📜
MIT 
