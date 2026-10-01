//! Cold Pass: a GNOME-style browser for Proton Drive, over the official
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
/// only warnings from the libraries.
const DEFAULT_LOG: &str = "warn,cold_pass=info";

fn main() -> iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG)),
        )
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .init();

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting Cold Pass");
    typography::set_monospace(typography::FONT_MONO);

    iced::application(app::App::new, app::App::update, app::App::view)
        .settings(typography::settings())
        .title("Cold Pass")
        .subscription(app::App::subscription)
        .theme(app::App::theme)
        .window_size((960.0, 640.0))
        .decorations(false)
        .transparent(true)
        .style(|_, theme| window::style(theme))
        .run()
}
