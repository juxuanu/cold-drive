//! Cold Pass: a GNOME-style browser for Proton Drive, over the official
//! `proton-drive` command-line client.

mod app;
mod drive;
mod files;
mod format;
mod icons;

use libadwaita_iced::{typography, window};

fn main() -> iced::Result {
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
