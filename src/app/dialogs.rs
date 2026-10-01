//! The preferences and about dialogs.

use libadwaita_iced::widget::about_dialog;
use libadwaita_iced::widget::view_switcher::page;
use libadwaita_iced::widget::{
    action_row, button_row, entry_row, preferences_dialog, preferences_group, preferences_page,
    spinner,
};
use libadwaita_iced::{Element, Widget, icons};

use super::{App, Dialog, Message};
use crate::drive::{self, Source};
use crate::{format, icons as more_icons};

const REPOSITORY: &str = "https://github.com/juxuanu/cold-pass";

impl App {
    pub(super) fn dialog_view(&self, which: Dialog) -> Element<'_, Message> {
        match which {
            Dialog::Preferences => self.preferences(),
            Dialog::About => self.about(),
        }
    }

    fn preferences(&self) -> Element<'_, Message> {
        let general = preferences_page()
            .push(self.cli_group())
            .push(self.account_group());

        preferences_dialog()
            .push(page("General").icon(icons::preferences_system()), general)
            .on_close(Message::CloseDialog)
            .boxed()
    }

    fn cli_group(&self) -> impl Widget<Message> + '_ {
        let overridden = std::env::var_os(drive::CLI_ENV).is_some_and(|value| !value.is_empty());

        let saved = self
            .config
            .cli_path
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        // The apply button shows only once there is something to apply.
        let changed = self.cli_path_input.trim() != saved;

        let path = entry_row("Path to proton-drive", &self.cli_path_input)
            .on_input(Message::CliPathInput)
            .on_submit(Message::ApplyCliPath)
            .on_apply(changed.then_some(Message::ApplyCliPath));

        let in_use = match &self.cli {
            Some(cli) => {
                let from = match cli.source() {
                    Source::Environment => format!("From {}", drive::CLI_ENV),
                    Source::Settings => "From the path above".to_owned(),
                    Source::Path => "Found on PATH".to_owned(),
                };
                info_row("In Use", format!("{}\n{from}", cli.program().display()))
            }
            None => info_row("In Use", "None found".to_owned()),
        };

        let version = match &self.cli_version {
            None if self.cli.is_some() => action_row("Version").suffix(spinner()),
            None => info_row("Version", "Unknown".to_owned()),
            Some(Ok(version)) => info_row("Version", version.clone()),
            Some(Err(error)) => info_row("Version", error.clone()),
        };

        let mut group = preferences_group()
            .title("Proton Drive CLI")
            .description(
                "Cold Pass runs the official Proton Drive CLI for everything it does. Leave the \
                 path empty to use the one on PATH.",
            )
            .push(path);
        if overridden {
            group = group.push(info_row(
                "Overridden",
                format!("{} is set, and takes precedence", drive::CLI_ENV),
            ));
        }

        group.push(in_use).push(version)
    }

    fn account_group(&self) -> impl Widget<Message> + '_ {
        let group = preferences_group().title("Account");

        if self.cli.is_none() || self.signed_out {
            return group.push(info_row(
                "Signed Out",
                "Sign in from the main window".to_owned(),
            ));
        }

        match &self.account {
            None => group.push(action_row("Email").suffix(spinner())),
            Some(Err(error)) => group.push(info_row("Could Not Read the Account", error.clone())),
            Some(Ok(account)) => group
                .push(info_row(
                    "Email",
                    account
                        .email
                        .clone()
                        .unwrap_or_else(|| "Unknown".to_owned()),
                ))
                .push(info_row(
                    "My Files",
                    format!(
                        "{} in {} items, trash included",
                        format::size(account.used),
                        account.items
                    ),
                ))
                .push(
                    button_row("Log Out")
                        .destructive()
                        .on_activate(Message::LogOut),
                ),
        }
    }

    fn about(&self) -> Element<'_, Message> {
        let cli = self.cli.as_ref().map_or_else(
            || "not found".to_owned(),
            |cli| cli.program().display().to_string(),
        );
        let version = match &self.cli_version {
            Some(Ok(version)) => version.as_str(),
            _ => "unknown",
        };
        let debug_info = format!(
            "Cold Pass {}\nCLI: {cli}\nCLI version: {version}\nOS: {} {}\n",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
        );

        about_dialog::about_dialog()
            .application_icon(more_icons::folder_remote())
            .application_name("Cold Pass")
            .developer_name("juxuanu")
            .version(env!("CARGO_PKG_VERSION"))
            .comments(
                "Browse Proton Drive in a GNOME-style window, through the official Proton Drive \
                 command-line client.",
            )
            .website(REPOSITORY)
            .issue_url(format!("{REPOSITORY}/issues"))
            .debug_info(debug_info)
            .copyright("© 2026 juxuanu")
            .license("Distributed under the MIT or the Apache 2.0 license, at your option.")
            .acknowledgement_section(
                "Built On",
                [
                    "libadwaita-iced https://gitlab.com/juxuanu/libadwaita-iced",
                    "iced https://iced.rs",
                    "Proton Drive CLI https://github.com/ProtonDriveApps/sdk",
                ],
            )
            .legal_section(
                "Adwaita Icons",
                "© The GNOME Project",
                about_dialog::License::Lgpl30,
                "",
            )
            .pages(self.about_pages.iter().copied())
            .on_push(Message::AboutPush)
            .on_pop(Message::AboutPop)
            .on_close(Message::CloseDialog)
            .on_activate_link(Message::OpenExternally)
            .on_copy(Message::CopyText)
            .toasts(&self.toasts, Message::ToastDismissed)
            .boxed()
    }
}

/// A row showing a value: the title as a dimmed caption over it, as
/// libadwaita's `.property` rows read.
fn info_row<'a>(
    title: &'a str,
    value: String,
) -> libadwaita_iced::widget::action_row::ActionRow<'a, Message> {
    action_row(title).subtitle(value).property(true)
}
