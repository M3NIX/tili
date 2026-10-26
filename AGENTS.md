# Development notes for tili

## Scope and conventions

These instructions apply throughout the repository.

- Keep tili a small foreground Linux terminal tiling manager, using stable Rust,
  edition 2024, small modules, and threads/channels rather than an async framework.
- Preserve working keyboard, mouse, clipboard, pane, and workspace behavior.
  Add focused regression tests when changing these paths.
- Test existing features, concrete bug regressions, and meaningful edge cases.
  Do not add tests merely to assert that removed or unimplemented features are
  absent. Keep negative tests for validation, safety, and failure handling.
- Use portable-pty for process terminals, tastty-core for terminal emulation and
  selection, crossterm for host terminal I/O, and nucleo-matcher for fuzzy search.
  Do not recreate terminal parsing or selection logic in application code.
- Keep dependencies minimal. Do not add daemons, session persistence, a plugin
  runtime, graphical backends, or a general Run launcher.
- Keep README.md short and user-facing. Maintain implementation details here.
  Document terminal capabilities generically, not around one personal test setup.
- Check actual dependency APIs before using them. Run the validation commands
  below for code changes; documentation-only changes need a diff/link review.

## Installation and maintenance

Requirements: Linux, stable Rust, and the system OpenSSH client for connections.
The SSH picker is native Rust and does not depend on Python. Build with `cargo build --release`;
install with `cargo install --path .`.

The default launcher mode uses `builtin = "ssh"`. No script is installed on startup.
For configurations from older versions, replace the SSH mode's
`command = "~/.config/tili/scripts/ssh-hosts.py"` with `builtin = "ssh"`.
Do not silently rewrite user configurations or delete previously installed scripts.
Custom modes continue using `command = "/path/to/script"` and the unchanged JSON
protocol. Each mode must specify exactly one of `builtin` or `command`.

## Key bindings

Alt is the default modifier. Preserve the custom J/K/L/Semicolon directional
layout rather than substituting conventional Vim navigation.

| Binding | Action |
| --- | --- |
| Alt+Enter | Open a shell pane |
| Alt+T | Open a shell using the focused process working directory when detectable |
| Alt+Backspace | Close the focused pane |
| Alt+H / Alt+V | Select horizontal / vertical splitting |
| Alt+E | Toggle split orientation |
| Alt+F | Toggle focused-pane fullscreen |
| Alt+D | Open the launcher |
| Alt+J / K / L / Semicolon | Focus left / down / up / right |
| Alt+Shift+J / K / L / Semicolon | Move left / down / up / right |
| Alt+R | Enter resize mode |
| Alt+1 ... Alt+0 | Switch to workspace 1 ... 10 |
| Alt+Shift+1 ... Alt+Shift+0 | Move the focused pane to workspace 1 ... 10 |
| Alt+Shift+C | Reload configuration |

Resize mode uses J/Left to shrink width, K/Down to grow height, L/Up to shrink
height, and Semicolon/Right to grow width. Enter, Escape, or Alt+R exits the
mode.

Like i3, panes inserted with the same orientation become equal siblings in that
container: two panes receive halves, three receive thirds, and four receive
quarters. Selecting the perpendicular orientation still creates a nested split.
Opening or closing a same-orientation sibling redistributes that container
equally; explicit resize actions can then adjust its ratios.
The focused pane previews the selected split with a cyan right edge for a
horizontal split or a cyan bottom edge for a vertical split.

Keyboard-selected focus wins after opening, closing, moving, or explicitly
focusing a pane. Focus follows the pointer only when the terminal sends an
actual mouse-motion event, so a stationary pointer does not override keyboard
focus. Clicking without moving the pointer does not change keyboard focus.
Rendering uses a retained cell buffer and updates only changed cells; ordinary
pane and launcher changes do not clear the screen.

Drag across pane text to select it. Only populated text cells are highlighted,
and an entirely empty selection is discarded. `tastty-core` owns selection
anchors, scrollback coordinates, multiline extraction, wide characters, and
soft-wrap handling; tili does not reconstruct selected text itself. Releasing
the button writes the selection to the local
Linux X11 clipboard when available. Tili retains one clipboard owner for its
lifetime and reuses it for every copy. Pure Wayland clipboard packages are not
linked; when X11 is unavailable, tili uses the terminal clipboard instead. In
SSH sessions it uses tmux clipboard integration when available, otherwise the
OSC 52 support provided by crossterm, so a remote desktop clipboard is never
selected by mistake. Ctrl+Shift+C repeats the copy when the terminal forwards
that key; mouse release is the reliable copy trigger because many terminals
consume their own copy shortcut. Ctrl+Shift+V uses the host terminal's normal
paste binding, which arrives at tili as a bracketed paste event and is forwarded
to the focused pane. Text extraction, hard-line breaks, and soft-wrap joining
come from the terminal emulator rather than being reconstructed from rendered
cells.

The mouse wheel scrolls the history of the pane under the pointer without
changing keyboard focus. Applications that enable terminal mouse reporting
receive mouse events directly instead; hold Shift while dragging to select text
from such an application.
The one-row top bar shows i3-style numbered boxes for non-empty workspaces and
highlights the active workspace.
Stacked panes share a single horizontal border row so their visual separation
matches the compact spacing between side-by-side panes.

## Launcher

The centered launcher loads entries once when a mode is entered and fuzzy
filters the cached entries in Rust. Ctrl+R refreshes, Ctrl+Tab and
Ctrl+Shift+Tab change modes, arrows select, Enter runs the selection, and
Escape closes the overlay. Underlying PTYs keep running.

Launcher executables use this intentionally small protocol:

```console
menu-script list
menu-script select <entry-id>
```

`list` writes one JSON object per line:

```json
{"id":"production","label":"production","description":"SSH host"}
```

`select` writes exactly one action object. Supported actions are `open_pane`,
`execute`, `replace_pane`, and `close`. Commands and argument arrays are passed
directly to the operating system; tili never constructs a shell command line.
Only configure trusted scripts because they run with the current user's
privileges. Output is bounded, operations time out, and superseded requests are
cancelled. List/select scripts run in their own process group, which is stopped
when the request finishes, fails, times out, or is cancelled. The timeout also
covers descendants holding output pipes open. Scripts must not daemonize or
escape that process group; this cleanup is not a sandbox.

Failed pane creation leaves existing panes and focus intact. Replacement still
requires confirmation; tili first prepares the new PTY/process, then removes the
old pane, so a spawn failure does not destroy the existing session.

Background `execute` actions receive no terminal input and discard stdout and
stderr, so they cannot interfere with pane rendering. Their exit status is
reported in tili. Use `open_pane` when command output or interaction is needed.

The SSH mode reads concrete aliases from `~/.ssh/config`, including recursive
`Include` directives. Entries show `alias (HostName)` when the concrete Host block
specifies a hostname, or just the alias otherwise. Search is fuzzy and
case-insensitive across the entire label; matched characters are highlighted in
bold yellow, independently of the selected row. Selecting still connects by alias.
Host discovery uses `ssh2-config` (default features disabled), not a hand-written
parser or an SSH protocol client. It supplies quoting, keyword handling, Include
globbing, cycle detection, and host scope matching. HostName values are displayed
literally, including matching defaults, without expanding tokens. Discovery supports
`Include` directives with relative paths and globs. It excludes wildcards and
negated patterns, deduplicates aliases, and never reads or displays key or
password values. It is an alias discovery tool, not a complete OpenSSH
configuration evaluator: `Match` conditions and aliases generated dynamically
by external tools are not evaluated or executed. HostName values in blocks with
unhandled Match directives are omitted rather than treated as unconditional.
Include cycles and parser errors are reported in the launcher.
Selecting an alias starts the system
`ssh` client in a new PTY, preserving normal agents, keys, ProxyJump, host-key
prompts, and interactive authentication.
Ordinary shell panes close normally after `exit` or Ctrl+D regardless of their
last command's status. Commands opened through the launcher close automatically
on success; launcher commands that fail leave their pane and terminal output
visible, together with their exit status, until closed with Alt+Backspace. This
keeps SSH name-resolution, configuration, authentication, and connection errors
available for diagnosis.

## Configuration

The optional path is `~/.config/tili/config.toml` (or
`$XDG_CONFIG_HOME/tili/config.toml`):

```toml
[general]
shell = "/bin/bash"
default_split = "horizontal"
resize_step = 10

[appearance]
focused_border = "#3971ED"
unfocused_border = "#282a2e"

[launcher]
default_mode = "ssh"
width_percent = 60
max_results = 10
timeout_ms = 3000

[[launcher.modes]]
name = "ssh"
builtin = "ssh"

[[launcher.modes]]
name = "docker"
command = "~/.config/tili/scripts/docker-menu"
```

Reloading changes the shell used by future panes, selected default split,
resize step, border colors, and launcher modes. Existing panes and PTYs remain
untouched. Invalid reloads are reported without terminating the application.

## Runtime behavior

- Each pane owns a `portable-pty` session and a `tastty-core` parser; child
  escape sequences are never forwarded directly to the host terminal.
- PTY sizes follow pane content rectangles and never become zero.
- A process exit removes only its pane. Closing the final pane in the active
  workspace returns to the most recently used non-empty workspace. Closing the
  final pane across all workspaces exits tili.
- Empty workspaces create a shell when activated. Background workspace PTYs
  continue running without causing unnecessary pane redraws. Moving the final
  pane away from the active workspace creates a replacement shell there.
- Raw mode, mouse reporting, bracketed paste, cursor visibility, line wrapping,
  and the alternate screen are restored through the renderer's drop guard on
  normal and handled-signal exits.

`tastty-core` handles common ANSI/VT behavior, alternate screens, colors,
cursor state, application cursor keys, bracketed paste, UTF-8, wide cells,
scrollback, mouse-reporting modes, and wrap-preserving primary-screen reflow.
Alternate-screen applications are resized geometrically and are expected to
redraw after SIGWINCH. Image protocols are not rendered. SIGKILL and hard
connection loss cannot guarantee user-space cleanup.

Parent-container focus, stacking, and tabbed layouts are deliberately deferred.
Supporting them correctly requires a broader container model that conflicts
with the project's minimal binary tiling design.

## Pane colors

Pane contents inherit host default foreground/background through SGR 39/49 and
preserve ANSI indices 0–255. Explicit truecolor remains RGB. UI colors (including
selection) stay explicit. Inverse cells emit SGR 7 after resolving colors in their
original roles; DEC reverse video combines with cell inverse using XOR.

Each parser starts with symbolic default foreground/background. OSC 10/11 values
are rendered as pane-local RGB; OSC 110/111 and RIS restore inheritance. Palette
entries whose resolved values differ from tastty-core's baseline are rendered as
RGB, so OSC 4 changes never modify the host palette. OSC 104 and RIS restore index
passthrough. The baseline is obtained from tastty-core rather than duplicated.

Limitations of tastty-core 0.1.1: its screen exposes resolved palette values, not
an override mask. An explicit OSC 4 assignment equal to the built-in xterm value
is therefore treated as an inherited index, even if the host palette differs.
Tracking only palette events cannot reliably fix this because RIS replaces the
screen and pending events without a distinct palette-reset event. OSC 4 currently
processes only the first index/color pair in a single command. No host palette
queries are performed: palette query replies use the emulator's palette, and
queries for unknown inherited default colors are left unanswered rather than
misreported as black. Applications choosing RGB themes from such queries may
therefore differ from direct execution. Truecolor, indexed colors, bold/dim, and
inverse ultimately depend on host terminal support and policy.

## Terminal protocol limitations

Legacy terminals encode Alt as an Escape prefix and may not distinguish every
Alt+Shift combination. tili accepts both shifted digits and their legacy symbol
forms for workspace movement, and supports arrow-key alternatives for
directional operations. Exact behavior still depends on the local terminal,
SSH client, keyboard layout, and extended-keyboard protocol support; universal
modified-key support is not claimed.

Ctrl+Shift+V is normally consumed by the host terminal rather than transmitted
as a key combination. It works when that terminal sends pasted text using
bracketed-paste mode, as modern terminals normally do. In a local X11 session
tili writes through the native clipboard API. In headless, pure Wayland, or SSH
sessions it falls back to OSC 52, which may be disabled by terminal policy or
an enclosing multiplexer. Inside tmux, tili first uses `tmux load-buffer -w` and
falls back to crossterm's OSC 52 command when forwarding is unavailable. Empty
selections never invoke a clipboard backend.

## Validation

GitHub Actions runs formatting, Clippy, tests, and a release build on Linux
x86_64 and ARM64 for pull requests and pushes to main/master. CI can also be
started manually. Python 3 is used only by external-script and PTY test fixtures and is
provided by the Ubuntu runners.

Pushing a `v*` or `release-*` tag triggers `.github/workflows/release-artifact.yml`.
It tests and packages Linux x86_64 glibc, ARM64 glibc, and x86_64 musl binaries,
then attaches archives and SHA-256 checksums to a GitHub Release. Only the publish
job has write permissions. glibc builds use Ubuntu 24.04; the musl build is
checked for static linking. There is no native Windows build: SSH clients can
run on other operating systems, but tili itself runs on Linux.
The native SSH mode runs `tili --ssh-picker list|select` in a supervised helper
process, reusing launcher cancellation, timeouts, output limits, and process-group
cleanup. It does not install or invoke a Python script. External script modes
and their `list`/`select` JSON protocol remain unchanged.
Hosted workflows must be verified on GitHub; local checks cannot validate runner
availability, repository permissions, or release publication.

```console
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build
cargo build --release
```

Manual terminal compatibility checklist (local and over SSH):

1. Connect with `ssh -t user@server tili` or start `tili` after login.
2. Exercise shell input, Ctrl+C/D/Z, bracketed paste, Neovim, htop, less, and ssh.
3. Open, focus, move, resize, fullscreen, and close several panes.
4. Leave the pointer stationary while using keys, then move it across panes and
   confirm focus changes only after motion.
5. Select and copy text, paste with Ctrl+Shift+V, and wheel-scroll both focused
   and unfocused panes; verify Shift-drag selection in a mouse-aware TUI.
6. Open the launcher, navigate rapidly, resize the host terminal, and confirm
   borders do not disappear or flash.
7. Select an SSH alias and verify interactive authentication and clean exit.
8. Move panes across all ten workspaces and verify background work continues.
9. Close the final pane and confirm the calling shell remains usable.

Automated tests do not establish compatibility with every terminal or SSH
client. Report the environments actually tested; never claim manual terminal
or SSH validation without performing it.

## Lifecycle and state invariants

- Reject launcher results after cancellation, closing, or an earlier completion.
- Keep the selected result visible beyond the first page and in short terminals.
- Disconnect background launcher commands from the application's terminal streams.
- Preserve explicit launcher settings when filling in an empty mode list.
- Remove the redundant path deserializer and renderer lifetime flag; reuse the
  workspace-count constant in the renderer.
- Acquire PTY I/O handles before spawning and establish session cleanup before
  starting the reader thread, covering partial startup failures.

## Resource and rendering invariants

- Script requests use nonblocking, bounded pipe reads under one deadline, with
  no output-reader threads to leak. A dedicated process group is terminated on
  every completion path. The leader remains unreaped until group cleanup to
  prevent PID reuse during descendant cleanup. Tests cover cancellation and
  timeout with both live/exited parents, inherited pipes, output limits, and
  nonzero exits. Deliberately daemonized descendants are outside this guarantee.
- Event queues are bounded to 128 messages each. PTY producers apply backpressure
  rather than dropping terminal bytes. Rendering batches up to 64 queued events
  or eight milliseconds of handling work. Clipboard operations still run after
  each event. Shutdown disconnects the receiver before joining input/signal
  threads, including startup failures. Tests transfer two MiB across four busy
  PTYs through a two-message queue and exercise saturated-queue shutdown.
- Overlay text uses unicode-segmentation and unicode-width (already transitive
  dependencies) for grapheme layout, clipping, and alignment. Control characters
  are removed before output. Tests cover wide text, combining accents, emoji,
  clipping, and overwriting wide cells. Pane text remains parsed by tastty-core.
- Recoverable spawn errors produce notices instead of terminating tili. Opening
  and replacing panes preserve the original pane/focus/fullscreen on spawn
  failure; workspace operations roll back when their new shell cannot start.
  Regression tests also verify that the original process remains alive.

Host terminals may consume Ctrl+Shift+C for their own selection instead of
forwarding the key to tili. Pane selection and host-terminal selection are
separate; copying on mouse release does not depend on receiving that shortcut.
