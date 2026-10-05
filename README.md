# Cold Pass

A GNOME-style Proton Drive browser, built with
[libadwaita-iced](https://gitlab.com/juxuanu/libadwaita-iced) on top of the
official [Proton Drive CLI](https://github.com/ProtonDriveApps/sdk/blob/main/cli/README.md).

- Browse My Files, Computers, Shared with Me, Shared by Me and Trash.
- Open text files and images inside the app (read-only, with select and copy;
  images zoom and pan).
- Open everything else in the desktop's default application.
- Upload files (Ctrl+U) and create folders (Ctrl+N) from the + menu beside the
  title; what arrives is selected. A file whose name is
  taken is uploaded under another name rather than replacing it.
- A click selects, Ctrl+click adds to the selection, Shift+click selects a
  run and a drag rubberbands; the arrows move the selection and Ctrl+A takes
  everything. A double click or Enter opens. A right click offers Download…
  (the selection, into a folder you choose) and Info.
- List or grid view, remembered between runs.
- Keyboard shortcuts in a dialog (Ctrl+?).
- Sign in through the browser; the preferences show the account and the CLI in
  use, set the CLI's path and log out.
- No thumbnails: the CLI cannot fetch the ones Proton stores, so files show
  their type's icon.

## Requirements

Install `proton-drive` from <https://proton.me/download/drive/cli> and put it
on your `PATH`, or set its path in the preferences. `COLD_PASS_DRIVE_CLI`
overrides both.

```sh
cargo run --release
```

## Logging

Logs go to stderr: every CLI call with its duration, and failures with what
the CLI said. `RUST_LOG` sets the level, `info` for Cold Pass by default:

```sh
RUST_LOG=cold_pass=debug cargo run
```

## How it works

Each action runs one `proton-drive … --json` command and parses its output.
Nodes are addressed as `/my-files/<uid>`, which the CLI resolves with a direct
lookup, so deep folders don't need a name lookup at every level.

Opened files are downloaded, decrypted, into `$XDG_CACHE_HOME/cold-pass/files`,
one directory per revision. That directory is wiped at startup and at log-out.
