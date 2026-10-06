# Cold Drive

A GNOME-style Proton Drive browser, built with
[libadwaita-iced](https://gitlab.com/juxuanu/libadwaita-iced) on top of the
official [Proton Drive CLI](https://github.com/ProtonDriveApps/sdk/blob/main/cli/README.md).

- Browse My Files, Computers, Shared with Me, Shared by Me and Trash.
- Open text files and images inside the app (read-only, with select and copy;
  images zoom and pan).
- Open everything else in the desktop's default application.
- Upload files (Ctrl+U) and create folders (Ctrl+N) from the + menu beside the
  title; what arrives is selected and scrolled into view. A file whose name is
  taken is uploaded under another name rather than replacing it.
- A click selects, Ctrl+click adds to the selection, Shift+click selects a
  run and a drag rubberbands; the arrows move the selection and Ctrl+A takes
  everything. A double click or Enter opens. A right click offers Download…
  (the selection, into a folder you choose), Rename… (F2), Move to Trash
  (Delete, undone from the toast) and Info; in the Trash, Restore and Delete
  Permanently…, behind a confirmation. The banner on the Trash page empties
  it, behind one too.
- Copy (Ctrl+C) or Cut (Ctrl+X) the selection and Paste (Ctrl+V) it into the
  folder shown, from the menu on a right click where no row is, or into a
  folder on the page from its own menu, or from the banner that says what
  was taken, shown until the first paste or its close. A cut pastes as a
  move.
- List or grid view, remembered between runs.
- Keyboard shortcuts in a dialog (Ctrl+?).
- Sign in through the browser; the preferences show the account and the CLI in
  use, set the CLI's path and log out.
- No thumbnails: the CLI cannot fetch the ones Proton stores, so files show
  their type's icon.

## Requirements

Install `proton-drive` from <https://proton.me/download/drive/cli> and put it
on your `PATH`, or set its path in the preferences. `COLD_DRIVE_PROTON_DRIVE_CLI`
overrides both.

```sh
cargo run --release
```

## Logging

Logs go to stderr: every CLI call with its duration, and failures with what
the CLI said. `RUST_LOG` sets the level, `info` for Cold Drive by default:

```sh
RUST_LOG=cold_drive=debug cargo run
```

## How it works

The quick commands — listings, info, sharing, trash, rename, copy, move —
go to the CLI's own interactive shell (`proton-drive` with no arguments),
kept running and given one `… --json` line at a time, so the CLI starts and
opens its cache once. Uploads, downloads and signing in run as commands of
their own, since they run long and are killed to cancel. Nodes are addressed
as `/my-files/<uid>`, which the CLI resolves with a direct lookup, so deep
folders don't need a name lookup at every level.

Opened files are downloaded, decrypted, into `$XDG_CACHE_HOME/cold-drive/files`,
one directory per revision. That directory is wiped at startup and at log-out.
