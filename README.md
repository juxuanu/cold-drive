# Cold Pass

A GNOME-style Proton Drive browser, built with
[libadwaita-iced](https://gitlab.com/juxuanu/libadwaita-iced) on top of the
official [Proton Drive CLI](https://github.com/ProtonDriveApps/sdk/blob/main/cli/README.md).

- Browse My Files, Computers, Shared with Me, Shared by Me and Trash.
- Open text files and images inside the app (read-only, with select and copy;
  images zoom and pan).
- Open everything else in the desktop's default application.
- Sign in through the browser and log out from the app.

## Requirements

Install `proton-drive` from <https://proton.me/download/drive/cli> and put it
on your `PATH`, or set `COLD_PASS_DRIVE_CLI` to where it is.

```sh
cargo run --release
```

## How it works

Each action runs one `proton-drive … --json` command and parses its output.
Nodes are addressed as `/my-files/<uid>`, which the CLI resolves with a direct
lookup, so deep folders don't need a name lookup at every level.

Opened files are downloaded, decrypted, into `$XDG_CACHE_HOME/cold-pass/files`,
one directory per revision. That directory is wiped at startup and at log-out.

Icons not embedded by libadwaita-iced come from adwaita-icon-theme
(`assets/icons`, LGPL-3.0 or CC-BY-SA-3.0).
