//! Cold Drive: a GNOME-style browser for Proton Drive, over the official
//! `proton-drive` command-line client.

mod app;
mod config;
mod drive;
mod files;
mod format;

use std::io::IsTerminal;

use libadwaita_iced::{typography, window};
use tracing_subscriber::EnvFilter;

/// What is logged unless `RUST_LOG` says otherwise: every CLI call, and
/// only warnings from the libraries — but `sctk_adwaita`'s. winit builds its
/// decoration frame on Wayland even with decorations off, only to hide it,
/// and the frame warns about every button in GNOME's `button-layout` it does
/// not draw, such as `icon`.
const DEFAULT_LOG: &str = "warn,sctk_adwaita=error,cold_drive=info";

fn main() -> iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG)),
        )
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .init();

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting Cold Drive");
    typography::set_monospace(typography::FONT_MONO);

    iced::application(app::App::new, app::App::update, app::App::view)
        .settings(typography::settings())
        .title("Cold Drive")
        .subscription(app::App::subscription)
        .theme(app::App::theme)
        .window_size((960.0, 640.0))
        .decorations(false)
        .transparent(true)
        .style(|_, theme| window::style(theme))
        .run()
}
