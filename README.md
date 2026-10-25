# tili

A lightweight, keyboard-driven terminal tiling manager inspired by i3.
Run independent shells and interactive applications in tiled panes, locally or
over SSH—no daemon, graphical environment, or client-side companion required.

Unlike i3, which tiles graphical windows inside an X11/Wayland session, tili
tiles PTY-backed terminal panes inside a single terminal window. It is for
managing many shells and TUIs in one terminal, especially over SSH, without a
multiplexer or desktop environment. For example, if you have to work on Windows
and can only reach a Linux server through one predetermined SSH client, run tili
there and still get the i3-style workflow.

## Features

- i3-style splits, directional navigation, resizing, and fullscreen panes.
- Ten workspaces with a compact workspace bar.
- A scriptable launcher with case-insensitive fuzzy search and highlighted matches.
- Native Rust SSH host discovery from `~/.ssh/config`.
- Mouse selection, copy-on-release, and per-pane scrollback.

## Install

Requires Linux and a stable Rust toolchain. The native SSH launcher uses the
system OpenSSH client; Python is not required.

From a checkout of this repository:

```sh
cargo install --path .
tili
```

Or start it on a remote machine where tili is installed:

```sh
ssh -t user@server tili
```

## Key bindings

| Keys                        | Action                                                             |
| --------------------------- | ------------------------------------------------------------------ |
| Alt+Enter                   | New shell pane                                                     |
| Alt+T                       | New shell in the focused pane's working directory, when detectable |
| Alt+Backspace               | Close pane                                                         |
| Alt+H / Alt+V               | Next split: side by side / above and below                         |
| Alt+E                       | Toggle split orientation                                           |
| Alt+J / K / L / Semicolon   | Focus left / down / up / right                                     |
| Alt+Shift + direction       | Move pane                                                          |
| Alt+R                       | Resize mode; use the same directions, then Enter or Escape         |
| Alt+F                       | Toggle fullscreen                                                  |
| Alt+1…9 / Alt+0             | Switch to workspace 1…9 / 10                                       |
| Alt+Shift+1…9 / Alt+Shift+0 | Move pane to workspace                                             |
| Alt+D                       | Open launcher                                                      |
| Alt+Shift+C                 | Reload configuration                                               |

Arrow keys also work for directional navigation. Closing the last pane in a
workspace returns to the previous open workspace; closing the final pane exits.

## SSH launcher

Press **Alt+D**, type to filter, and press **Enter** to connect in a new pane.
Hosts appear as `test1 (test1.localhost.myydomain)` when a HostName is configured.
Use Up/Down to select, Ctrl+R to refresh, and Escape to close.

Additional launcher modes can use your own executable scripts. Configure only
trusted scripts: they run with your user's permissions.

For a minimal Python example, copy `scripts/example-menu.py` to your scripts
directory, make it executable, and add a mode:

```sh
install -Dm755 scripts/example-menu.py ~/.config/tili/scripts/example-menu.py
```

```toml
# ~/.config/tili/config.toml
[[launcher.modes]]
name = "examples"
command = "~/.config/tili/scripts/example-menu.py"
```

External modes use `script list` for JSON Lines entries and
`script select <id>` for one JSON action response.

## Configuration

No configuration is required. Optional settings live at
`~/.config/tili/config.toml`, or `$XDG_CONFIG_HOME/tili/config.toml`.

See [AGENTS.md](AGENTS.md) for configuration examples, the script protocol,
development notes, and testing guidance.

Keyboard modifiers and clipboard access depend on the host terminal's
capabilities and settings, including when used over SSH. Session persistence,
tabbed layouts, and stacking are not currently supported.
