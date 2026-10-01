//! Symbolic icons from adwaita-icon-theme that libadwaita_iced does not
//! embed; see `assets/icons/SOURCE` for the revision and `COPYING` for the
//! licence.

use iced::widget::svg::Handle;

macro_rules! icons {
    ($($name:ident => $file:literal,)*) => {
        $(
            pub fn $name() -> Handle {
                Handle::from_memory(
                    include_bytes!(concat!("../assets/icons/", $file, "-symbolic.svg")).as_slice(),
                )
            }
        )*
    };
}

icons! {
    text_x_generic => "text-x-generic",
    image_x_generic => "image-x-generic",
    audio_x_generic => "audio-x-generic",
    video_x_generic => "video-x-generic",
    package_x_generic => "package-x-generic",
    x_office_document => "x-office-document",
    x_office_spreadsheet => "x-office-spreadsheet",
    x_office_presentation => "x-office-presentation",
    application_x_executable => "application-x-executable",
    folder => "folder",
    user_home => "user-home",
    folder_remote => "folder-remote",
    folder_publicshare => "folder-publicshare",
    network_workgroup => "network-workgroup",
    user_trash => "user-trash",
    computer => "computer",
    view_refresh => "view-refresh",
    edit_copy => "edit-copy",
    network_offline => "network-offline",
}
