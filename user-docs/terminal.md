# Terminal sessions

## Native GUI

Click the Terminal icon in the left activity bar, choose **View → Terminal**, or
press **Cmd+Shift+T** on macOS or **Ctrl+Shift+T** elsewhere to show or hide the
integrated terminal. The terminal opens an interactive shell in the current
workspace directory. Typing, paste, and terminal shortcuts go directly to the
shell while it has focus.

Hiding the panel preserves the running shell and its scrollback. Exit the shell
normally, then use **Restart terminal** to start a fresh session. Closing Ovim
closes the terminal session.

The integrated terminal is available in the native GUI; the browser development
preview does not start local processes.

## Terminal frontend

In the terminal frontend, use `:terminal` or `:term` to leave Ovim temporarily
and start your configured interactive shell:

```vim
:terminal
```

Ovim uses `SHELL` on Unix and `COMSPEC` on Windows, with `/bin/sh` and
`cmd.exe` as fallbacks. Exit the shell normally (`exit` or Ctrl-D on Unix) to
return to the same editor session.

You can also run a specific program or command:

```vim
:terminal lazygit
:term cargo test
```

`:shell` is an alias for opening the interactive shell. For a non-interactive
command whose output you want to inspect, continue to use `:!command`; Ovim
shows that output and waits for Enter before returning.

## Command scope

Terminal sessions temporarily use the terminal that launched Ovim. They are not
PTY-backed Ovim buffers, so there is no terminal scrollback buffer, split-window
terminal, or terminal-mode keymap yet. The TUI event loop is paused until the
child exits. The `:terminal` and `:shell` commands still belong to the terminal
frontend; use the Terminal icon or Cmd/Ctrl+Shift+T in the GUI. The headless
command API rejects interactive sessions.
