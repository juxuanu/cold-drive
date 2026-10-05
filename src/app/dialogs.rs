//! The preferences and about dialogs.

use iced::widget::{column, text};
use iced::{Alignment, Fill};
use libadwaita_iced::widget::about_dialog;
use libadwaita_iced::widget::view_switcher::page;
use libadwaita_iced::widget::{
    action_row, button_row, entry_row, preferences_dialog, preferences_group, preferences_page,
    spinner,
};
use libadwaita_iced::widget::{
    alert_dialog, header_bar, icon, response, shortcuts_dialog, shortcuts_item, shortcuts_section,
    toolbar_view, window_title,
};
use libadwaita_iced::{Element, Widget, icons, typography, widget as adw};

use super::{App, Dialog, Message};
use crate::drive::{self, Kind, Source};
use crate::{files, format};

/// The naming dialog's entry, focused as the dialog opens.
pub(super) const NAME_ENTRY: &str = "name-entry";

const REPOSITORY: &str = "https://github.com/juxuanu/cold-pass";

impl App {
    pub(super) fn dialog_view(&self, which: Dialog) -> Element<'_, Message> {
        match which {
            Dialog::Preferences => self.preferences(),
            Dialog::About => self.about(),
            Dialog::NewFolder => self.naming(),
            Dialog::Shortcuts => shortcuts(),
            Dialog::Info => self.info(),
            Dialog::Delete => self.delete_alert(),
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

        let group = match &self.account {
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
                .extend(account.usage.map(|usage| {
                    info_row(
                        "My Files",
                        format!(
                            "{} in {} items, trash included",
                            format::size(usage.bytes),
                            usage.items
                        ),
                    )
                })),
        };

        // Whatever could be read of the account, the session can end.
        group.push(
            button_row("Log Out")
                .destructive()
                .on_activate(Message::LogOut),
        )
    }

    /// Files' New Folder and Rename dialogs: a name, checked as it is
    /// typed, and the button that takes it.
    fn naming(&self) -> Element<'_, Message> {
        let Some(naming) = &self.naming else {
            return iced::widget::space().boxed();
        };
        let problem = self.name_problem();
        let valid = problem.is_ok();
        let why = problem.err().flatten();

        let (title, placeholder, button) = match &naming.rename {
            Some(_) => (format!("Rename {}", naming.kind()), "Name", "Rename"),
            None => ("New Folder".to_owned(), "Folder name", "Create"),
        };

        let entry = adw::text_input(placeholder, &naming.name)
            .id(NAME_ENTRY)
            .on_input(Message::NameTyped)
            .on_submit_maybe(valid.then_some(Message::NameSubmitted))
            .style(if why.is_some() {
                adw::text_input::error
            } else {
                adw::text_input::default
            });

        let mut content = column![entry].spacing(6);
        if let Some(why) = why {
            content = content.push(typography::caption(why).style(adw::text::error).boxed());
        }

        alert_dialog(title)
            .child(content)
            .response(response("Cancel", Message::CloseDialog))
            .response(
                response(button, Message::NameSubmitted)
                    .suggested()
                    .enabled(valid)
                    .default_response(),
            )
            .boxed()
    }

    /// Files' alert before a permanent delete: the item, or the count,
    /// in the heading, a Cancel that is the default, and a destructive
    /// Delete.
    fn delete_alert(&self) -> Element<'_, Message> {
        let heading = match self
            .deleting
            .as_ref()
            .map(|(_, entries)| entries.as_slice())
        {
            Some([entry]) => format!("Permanently Delete “{}”?", entry.name),
            Some(entries) => format!("Permanently Delete {} Selected Items?", entries.len()),
            None => "Permanently Delete?".to_owned(),
        };

        alert_dialog(heading)
            .body("Permanently deleted items can't be restored")
            .response(response("Cancel", Message::CloseDialog).default_response())
            .response(response("Delete", Message::DeleteForever).destructive())
            .boxed()
    }

    /// What there is to know of one item: its icon and name over the
    /// rows of a properties dialog, the CLI's details filled in as they
    /// come.
    fn info(&self) -> Element<'_, Message> {
        let Some(info) = &self.info else {
            return iced::widget::space().boxed();
        };
        let entry = &info.entry;

        let kind = match entry.kind {
            Kind::Folder => "Folder".to_owned(),
            Kind::Device => "Computer".to_owned(),
            Kind::File => match files::classify(entry) {
                files::Class::Document(document) => document.description().to_owned(),
                _ => entry
                    .media_type
                    .clone()
                    .unwrap_or_else(|| "File".to_owned()),
            },
        };

        let heading = column![
            icon(super::entry_icon(entry)).size(64),
            typography::title_4(entry.name.as_str())
                .center()
                .wrapping(text::Wrapping::WordOrGlyph),
            typography::caption(kind.clone()).style(adw::text::dimmed),
        ]
        .spacing(6)
        .align_x(Alignment::Center)
        .width(Fill);

        let mut rows = preferences_group()
            .push(info_row("Type", kind))
            .push(info_row("Location", info.location.clone()));

        rows = match &info.details {
            None => rows.push(action_row("Details").suffix(spinner())),
            Some(Err(error)) => rows.push(info_row("Could Not Read the Details", error.clone())),
            Some(Ok(details)) => {
                let date = |when: &Option<String>| when.as_deref().and_then(format::full_date);
                let shared = match (details.shared, details.shared_by_link) {
                    (_, true) => "Yes, by link",
                    (true, false) => "Yes",
                    (false, false) => "No",
                };

                rows.extend(
                    [
                        // The exact count, once the size is rounded.
                        details.size.map(|size| match size {
                            0..1000 => ("Size", format::size(size)),
                            _ => ("Size", format!("{} ({size} bytes)", format::size(size))),
                        }),
                        date(&details.modified).map(|date| ("Modified", date)),
                        date(&details.created).map(|date| ("Created", date)),
                        details.created_by.clone().map(|who| ("Created By", who)),
                        details.owner.clone().map(|owner| ("Owner", owner)),
                        Some(("Shared", shared.to_owned())),
                        details
                            .stored
                            .map(|stored| ("Stored on Proton Drive", format::size(stored))),
                        details.sha1.clone().map(|sha1| ("SHA-1", sha1)),
                    ]
                    .into_iter()
                    .flatten()
                    .map(|(title, value)| info_row(title, value)),
                )
            }
        };

        let page = adw::scrollable(column![heading, rows].spacing(24).padding([24, 18]));

        toolbar_view(page)
            .top(
                header_bar()
                    .title(window_title("Info"))
                    .on_close(Message::CloseDialog),
            )
            .width(420)
            .height(560)
            .boxed()
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
            .application_icon(icons::folder_remote())
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

/// Every shortcut the window answers, as `subscription` and the
/// navigation view take them.
fn shortcuts() -> Element<'static, Message> {
    shortcuts_dialog()
        .section(
            shortcuts_section("General")
                .item(shortcuts_item("Preferences", "<Control>comma"))
                .item(shortcuts_item("Keyboard Shortcuts", "<Control>question")),
        )
        .section(
            shortcuts_section("Files")
                .item(shortcuts_item("Upload Files", "<Control>u"))
                .item(shortcuts_item("New Folder", "<Control>n"))
                .item(shortcuts_item("Rename the Selected Item", "F2"))
                .item(
                    shortcuts_item("Move the Selection to Trash", "Delete")
                        .subtitle("In the Trash, deletes it permanently"),
                )
                .item(shortcuts_item("Refresh", "F5 <Control>r"))
                .item(shortcuts_item("Select All", "<Control>a"))
                .item(shortcuts_item(
                    "Clear the Selection",
                    "<Shift><Control>a Escape",
                ))
                .item(
                    shortcuts_item("Move the Selection", "Up Down Home End Page_Up Page_Down")
                        .subtitle("Ctrl moves alone, Shift extends"),
                )
                .item(shortcuts_item("Open the Selected Item", "Return")),
        )
        .section(
            shortcuts_section("Navigation")
                .item(shortcuts_item("Back", "<Alt>Left <Alt>Up Escape")),
        )
        .on_close(Message::CloseDialog)
        .boxed()
}

/// A row showing a value: the title as a dimmed caption over it, as
/// libadwaita's `.property` rows read.
fn info_row<'a>(
    title: &'a str,
    value: String,
) -> libadwaita_iced::widget::action_row::ActionRow<'a, Message> {
    action_row(title).subtitle(value).property(true)
}
