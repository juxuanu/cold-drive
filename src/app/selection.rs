//! How a selected grid item looks, after libadwaita's stylesheet: washed in
//! the accent — `$view_selected_color`, 25%, and 32% and 39% under the
//! pointer and pressed — keeping its text colour. A selected row the library
//! draws itself, `ListRow::selected`.

use iced::Border;
use libadwaita_iced::metrics::{radius, state};
use libadwaita_iced::widget::button::{Status, Style};
use libadwaita_iced::widget::support::{self, Surface};
use libadwaita_iced::{Adwaita, color};

/// The accent wash of a selected item, by how the pointer is on it.
fn selected_wash(theme: &Adwaita, status: Status) -> iced::Color {
    let opacity = match status {
        Status::Active | Status::Disabled => state::VIEW_SELECTED,
        Status::Hovered => state::VIEW_SELECTED_HOVER,
        Status::Pressed => state::VIEW_SELECTED_ACTIVE,
    };

    color::alpha(theme.colors().accent_bg, opacity)
}

/// A child of an icon view — `gridview > child`: `$button_radius` round,
/// `$view_hover_color` and `$view_active_color` under the pointer, the
/// accent wash when selected, outlined in high contrast.
pub fn tile(theme: &Adwaita, status: Status, selected: bool) -> Style {
    let surface = Surface::Window;
    let is_disabled = matches!(status, Status::Disabled);

    let background = if selected {
        Some(selected_wash(theme, status))
    } else {
        match status {
            Status::Active | Status::Disabled => None,
            Status::Hovered => Some(surface.overlay(theme, state::VIEW_HOVER)),
            Status::Pressed => Some(surface.overlay(theme, state::VIEW_ACTIVE)),
        }
    };

    let border = if selected && theme.is_high_contrast() {
        Border {
            color: surface.border(theme),
            width: 1.0,
            radius: radius::BUTTON.into(),
        }
    } else {
        Border {
            radius: radius::BUTTON.into(),
            ..Border::default()
        }
    };

    Style {
        background: background.map(Into::into),
        text_color: support::maybe_disabled(theme, surface.foreground(theme), is_disabled),
        border,
        ..Style::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::Background;

    fn alpha(style: &Style) -> Option<f32> {
        match style.background {
            Some(Background::Color(color)) => Some(color.a),
            _ => None,
        }
    }

    #[test]
    fn selection_washes_in_the_accent_at_libadwaitas_steps() {
        let theme = Adwaita::light();
        let accent = theme.colors().accent_bg;

        for (status, opacity) in [
            (Status::Active, state::VIEW_SELECTED),
            (Status::Hovered, state::VIEW_SELECTED_HOVER),
            (Status::Pressed, state::VIEW_SELECTED_ACTIVE),
        ] {
            let Some(Background::Color(wash)) = tile(&theme, status, true).background else {
                panic!("a selected item is washed");
            };
            assert_eq!((wash.r, wash.g, wash.b), (accent.r, accent.g, accent.b));
            assert!((wash.a - accent.a * opacity).abs() < 1e-6);
        }
    }

    #[test]
    fn unselected_items_keep_their_own_look() {
        let theme = Adwaita::light();
        assert_eq!(alpha(&tile(&theme, Status::Active, false)), None);
        assert!(alpha(&tile(&theme, Status::Hovered, false)).is_some());
    }
}
