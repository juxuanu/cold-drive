//! The window: a sidebar of Drive's sections beside a navigation stack of
//! folders, with files opened onto it as pages of their own.

use std::path::PathBuf;

use iced::keyboard::{self, Key, key::Named};
use iced::theme::Mode;
use iced::widget::{center, column, container, image, row, svg, text, text_editor};
use iced::{Alignment, Fill, Font, Subscription, Task};
use libadwaita_iced::widget::about_dialog::Page as AboutPage;
use libadwaita_iced::widget::breakpoint_bin::{self, breakpoint_bin};
use libadwaita_iced::widget::list_view::Selection;
use libadwaita_iced::widget::navigation_view::NavigationPage;
use libadwaita_iced::widget::popover_menu::Entry as MenuEntry;
use libadwaita_iced::widget::popover_menu::{item, menu_button, separator};
use libadwaita_iced::widget::sidebar::{self, Mode as SidebarMode};
use libadwaita_iced::widget::support::Surface;
use libadwaita_iced::widget::toast::{self, Toasts};
use libadwaita_iced::widget::{action_row as row_metrics, grid_view, list_view, preferences_group};
use libadwaita_iced::widget::{
    clamp, dialog, header_bar, icon, navigation_page, navigation_split_view, navigation_view,
    search_entry, spinner, status_page, toast_overlay, toolbar_view, window_title,
};
use libadwaita_iced::{
    AccentColor, Adwaita, ColorScheme, Contrast, Element, Widget, icons, metrics, typography,
    widget as adw, window,
};

use crate::config::{Config, View};
use crate::drive::{
    self, Account, Cli, Details, Done, Entry, Invitation, Kind, Login, Role, Sharing, Transfer,
};
use crate::files::{self, Class, Content, Document, Opened};
use crate::format;

mod dialogs;

/// A grid cell's widest; the columns are as many as fit.
const TILE_WIDTH: f32 = 128.0;

/// Where the CLI is to be had.
const CLI_DOWNLOAD: &str = "https://proton.me/download/drive/cli/index.html";

/// An operation under way — Files' `NautilusProgressInfo`: what it is,
/// a detail line under it, and the task it runs as, whose CLI child dies
/// with it when it is cancelled.
struct Operation {
    id: u64,
    status: String,
    details: String,
    /// The folder's page, listed again once the operation is cancelled.
    tag: String,
    /// Whether the folder counts it among its transfers.
    transfer: bool,
    /// Aborts the task once dropped; `None` until the task is made.
    handle: Option<iced::task::Handle>,
}

pub struct App {
    /// The operations running, oldest first.
    operations: Vec<Operation>,
    next_operation: u64,
    config: Config,
    cli: Option<Cli>,
    section: Section,
    /// Collapsed, whether the content is up rather than the sidebar.
    show_content: bool,
    /// A section was just opened: its root page replaces the stack at
    /// once, with no slide, until it reports itself shown.
    switching: bool,
    /// The content's navigation stack, bottom first. Popped pages stay until
    /// they have slid out of view.
    pages: Vec<Page>,
    next_tag: u64,
    /// The CLI has no session.
    signed_out: bool,
    sign_in: Option<SignIn>,
    toasts: Toasts<Message>,
    /// What a copy button just put on the clipboard, while it shows a
    /// check mark for it: "Link".
    copied: Option<&'static str>,
    scheme: ColorScheme,
    focused: bool,
    maximized: bool,
    /// The dialog presented, while it is up or still closing.
    dialog: Option<Dialog>,
    /// Whether the dialog is up; `false` while it animates away.
    dialog_open: bool,
    /// The preferences' CLI path entry, until applied.
    cli_path_input: String,
    /// `None` while being asked.
    cli_version: Option<Result<String, String>>,
    account: Option<Result<Account, String>>,
    about_pages: Vec<AboutPage>,
    naming: Option<Naming>,
    /// What the Delete alert is about: the page, and the items.
    deleting: Option<(String, Vec<Entry>)>,
    /// What Copy or Cut took, for Paste.
    clipboard: Option<Clipboard>,
    share: Option<Share>,
    info: Option<Info>,
}

/// What Copy or Cut took: the items, and whether Paste moves them — Files'
/// clipboard, kept here since the items are nodes of the account, not
/// files of the system.
#[derive(Debug, Clone)]
struct Clipboard {
    entries: Vec<Entry>,
    cut: bool,
    /// The folder they are in, for the operation's details.
    from: String,
    /// Whether the banner says so: until something is pasted, or it is
    /// dismissed. Paste still works after.
    shown: bool,
}

/// The item the Info dialog is about.
struct Info {
    entry: Entry,
    /// Where it is: the trail of the folder it is in.
    location: String,
    /// `None` while the CLI is asked.
    details: Option<Result<Details, String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialog {
    Preferences,
    About,
    NewFolder,
    Shortcuts,
    Info,
    /// Files' "Permanently Delete…?" alert.
    Delete,
    /// Files' "Empty Trash?" alert.
    EmptyTrash,
    Share,
}

/// The Share dialog: an item, who it is shared with, and the invitation
/// being written.
pub(super) struct Share {
    entry: Entry,
    /// `None` while the CLI is asked; `Ok(None)` for an item not shared.
    sharing: Option<Result<Option<Sharing>, String>>,
    email: String,
    role: Role,
    /// What the invitation email says, in clear text, if anything.
    message: String,
    /// Whether the email names the item, in clear text.
    include_name: bool,
    /// The link's password and expiry as being edited, applied on demand.
    link_password: String,
    link_expiry: String,
    /// The change under way at the CLI, with its spinner in its row.
    busy: Option<Busy>,
}

/// What the Share dialog is asking of the CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Busy {
    Invite,
    /// Removing this email from the share.
    Uninvite(String),
    /// Turning the public link on or off.
    Link,
    LinkRole,
    /// The link's password and expiry.
    LinkSettings,
}

impl Share {
    fn link(&self) -> Option<&drive::Link> {
        match &self.sharing {
            Some(Ok(Some(sharing))) => sharing.link.as_ref(),
            _ => None,
        }
    }

    /// The expiry as typed, as the CLI takes it: an ISO date, or none;
    /// `Err` for text that is not a date.
    fn expiry(&self) -> Result<Option<String>, ()> {
        let text = self.link_expiry.trim();
        if text.is_empty() {
            return Ok(None);
        }
        chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
            .map(|date| Some(date.to_string()))
            .map_err(|_| ())
    }
}

/// The folder being named in the New Folder dialog.
/// The naming dialog: a folder to create, or an item to rename.
struct Naming {
    /// The page of the folder it is in.
    tag: String,
    name: String,
    /// Asked for, so a second Create or Rename is not.
    busy: bool,
    /// The item being renamed; `None` for a new folder.
    rename: Option<Entry>,
}

impl Naming {
    /// What is being named, as the dialog words it.
    fn kind(&self) -> &'static str {
        match &self.rename {
            Some(entry) if !entry.is_folder() => "File",
            _ => "Folder",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    MyFiles,
    Devices,
    SharedWithMe,
    SharedByMe,
    Trash,
}

struct Page {
    tag: String,
    popped: bool,
    kind: PageKind,
}

enum PageKind {
    Folder(Folder),
    Viewer(Viewer),
}

struct Folder {
    title: String,
    /// Where the folder is, for the header bar: "My Files / Work".
    trail: String,
    path: String,
    listing: Listing,
    filter: String,
    /// The file being downloaded to open, by UID.
    opening: Option<String>,
    /// Whether files and folders can be added here.
    writable: bool,
    /// Uploads into the folder and downloads from it still running.
    transfers: usize,
    /// Names to select once the folder is next listed: what was just
    /// created or uploaded into it.
    select_next: Vec<String>,
    /// The invitations waiting, on the Shared with Me page.
    invitations: Vec<Invitation>,
    /// The entries selected, by their index among those shown — the
    /// view's selection, lent to it each frame.
    selected: Selection,
}

impl Folder {
    /// The entries shown, as the search leaves them.
    fn shown(&self) -> Vec<&Entry> {
        let Listing::Loaded(entries) = &self.listing else {
            return Vec::new();
        };
        let needle = self.filter.to_lowercase();

        entries
            .iter()
            .filter(|entry| needle.is_empty() || entry.name.to_lowercase().contains(&needle))
            .collect()
    }

    /// The entries selected, as far as they are shown.
    fn selected_entries(&self) -> Vec<&Entry> {
        let shown = self.shown();

        self.selected
            .iter()
            .filter_map(|index| shown.get(index).copied())
            .collect()
    }

    fn entry(&self, uid: &str) -> Option<&Entry> {
        match &self.listing {
            Listing::Loaded(entries) => entries.iter().find(|entry| entry.uid == uid),
            _ => None,
        }
    }
}

enum Listing {
    Loading,
    Loaded(Vec<Entry>),
    Failed(String),
}

struct Viewer {
    entry: Entry,
    file: PathBuf,
    body: Body,
}

enum Body {
    Text(text_editor::Content),
    Image(image::Handle),
    Svg(svg::Handle),
}

struct SignIn {
    url: Option<String>,
    /// Aborts `auth login` — and so kills the CLI — once dropped.
    _task: iced::task::Handle,
}

#[derive(Debug, Clone)]
pub enum Message {
    Window(window::Action),
    Focused(bool),
    Maximized(bool),
    Resized,
    SystemScheme(Mode),
    ToastDismissed(toast::Id),

    SectionSelected(usize),
    ContentShown(bool),
    Listed(String, Result<Vec<Entry>, drive::Error>),
    Filtered(String, String),
    Reload(String),
    RefreshTop,
    /// The view's selection, as the user made it.
    Selected(String, Selection),
    /// An item opened: a double click or Enter on it, by its index among
    /// those shown.
    Activated(String, usize),
    /// Cancel the operation, from its row in the popover.
    CancelOperation(u64),
    /// Move the selection on the page to the trash.
    Trash(String),
    Trashed(u64, String, Vec<Entry>, Result<Done, drive::Error>),
    /// Restore the items from the trash: the menu in the trash, or Undo.
    Restore(String, Vec<Entry>),
    Restored(u64, String, Vec<Entry>, Result<Done, drive::Error>),
    /// Ask before deleting the selection on the page for good.
    ConfirmDelete(String),
    DeleteForever,
    Deleted(u64, String, Vec<Entry>, Result<Done, drive::Error>),
    /// Ask before emptying the trash: the banner on its page.
    ConfirmEmptyTrash,
    EmptyTrash,
    TrashEmptied(u64, Result<(), drive::Error>),
    /// Delete: the trash on a folder page, for good in the trash.
    DeleteSelected,
    /// Copy or cut the selection on the page: Paste then copies or moves it.
    Copy(String),
    Cut(String),
    /// Ctrl+C and Ctrl+X, on the folder on top.
    CopySelected,
    CutSelected,
    /// Paste into the folder page; Ctrl+V, into the one on top.
    Paste(String),
    PasteHere,
    /// Paste into a folder on the page, by UID.
    PasteInto(String, String),
    /// The page to list again, the names to select on it, whether it was
    /// a move, and how it went.
    Pasted(u64, String, Vec<String>, bool, Result<Done, drive::Error>),
    /// Select every row shown on the page.
    SelectAll(String),
    /// A section's root page is on; pushes slide again.
    RootShown,
    /// Close the banner saying what was taken; the clipboard keeps it.
    DismissTaken,
    /// Open the Share dialog on the item.
    ShareIn(String, String),
    /// Leave the item shared with the account.
    Leave(String, String),
    Left(u64, String, String, Result<(), drive::Error>),
    /// The invitations waiting, for the page.
    Invitations(String, Result<Vec<Invitation>, drive::Error>),
    AcceptInvitation(String, Invitation),
    RejectInvitation(String, Invitation),
    /// An invitation answered: the page, the item, and how it went.
    InvitationAnswered(u64, String, String, Result<(), drive::Error>),
    /// The sharing as the CLI tells it, for the item by UID.
    ShareLoaded(String, Result<Option<Sharing>, drive::Error>),
    ShareEmail(String),
    ShareRole(Role),
    InviteText(String),
    ShareIncludeName(bool),
    Invite,
    Uninvite(String),
    LinkToggled(bool),
    LinkRole(Role),
    LinkPassword(String),
    LinkExpiry(String),
    /// Send the link's password and expiry as edited.
    LinkApply,
    /// Escape, outside a dialog: whatever is up first goes.
    Escape,
    Download(String),
    DownloadTo(String, Vec<String>, Option<PathBuf>),
    Downloaded(u64, String, Result<Transfer, drive::Error>),
    ShowInfo(String, String),
    InfoLoaded(String, Result<Details, drive::Error>),
    Opened(String, String, Result<Opened, drive::Error>),
    OpenExternally(String),
    OpenedExternally(Result<(), String>),
    Edited(String, text_editor::Action),
    Back,
    PageHidden(String),

    SignIn,
    Login(Login),
    CancelSignIn,
    /// Put the text on the clipboard; the button for it — named, as
    /// "Link" — shows a check mark for a second once it is there.
    CopyText(String, &'static str),
    Copied(&'static str),
    /// The second is up.
    CopyShown(&'static str),
    LogOut,
    LoggedOut(Result<(), drive::Error>),

    ToggleView,
    SettingsSaved(Result<(), String>),
    ShowDialog(Dialog),
    CloseDialog,
    DialogClosed,
    CliPathInput(String),
    ApplyCliPath,
    CliVersion(Result<String, drive::Error>),
    AccountLoaded(Result<Account, drive::Error>),
    AboutPush(AboutPage),
    AboutPop,

    UploadTo(String),
    UploadHere,
    Picked(String, Vec<PathBuf>),
    /// The folder's page, the files' names.
    Uploaded(u64, String, Vec<String>, Result<Transfer, drive::Error>),
    NewFolderIn(String),
    NewFolderHere,
    /// Rename the item, by the page it is on and its UID.
    RenameIn(String, String),
    /// Rename the one item selected, F2.
    RenameSelected,
    NameTyped(String),
    NameSubmitted,
    FolderCreated(u64, String, String, Result<(), drive::Error>),
    /// The page, the old name, the new name, and how it went.
    Renamed(u64, String, String, String, Result<(), drive::Error>),
}

impl App {
    pub fn new() -> (Self, Task<Message>) {
        // Decrypted copies are kept for one session only.
        let _ = std::fs::remove_dir_all(files::cache_dir_root());

        let config = Config::load();
        let cli = Cli::locate(config.cli_path.as_deref());
        let mut app = Self::blank(config, cli);

        let load = app.open_section(Section::MyFiles);
        (
            app,
            Task::batch([iced::system::theme().map(Message::SystemScheme), load]),
        )
    }

    /// The app before anything is shown.
    fn blank(config: Config, cli: Option<Cli>) -> Self {
        Self {
            cli,
            cli_path_input: config
                .cli_path
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            config,
            section: Section::MyFiles,
            show_content: false,
            switching: false,
            pages: Vec::new(),
            next_tag: 0,
            signed_out: false,
            sign_in: None,
            toasts: Toasts::new(),
            copied: None,
            scheme: ColorScheme::Light,
            focused: true,
            maximized: false,
            dialog: None,
            dialog_open: false,
            cli_version: None,
            account: None,
            about_pages: Vec::new(),
            naming: None,
            deleting: None,
            clipboard: None,
            share: None,
            operations: Vec::new(),
            next_operation: 0,
            info: None,
        }
    }

    pub fn theme(&self) -> Adwaita {
        Adwaita::new(self.scheme, Contrast::Normal, AccentColor::Purple)
    }

    /// What the app listens to: the window's focus and size, the system's
    /// colour scheme, and the keys no widget took. The events come through
    /// one subscription, as each one is a channel the runtime feeds every
    /// event, and four of them filled up on macOS.
    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            iced::system::theme_changes().map(Message::SystemScheme),
            iced::event::listen_with(|event, status, _window| match event {
                iced::Event::Window(iced::window::Event::Focused) => Some(Message::Focused(true)),
                iced::Event::Window(iced::window::Event::Unfocused) => {
                    Some(Message::Focused(false))
                }
                iced::Event::Window(iced::window::Event::Resized(_)) => Some(Message::Resized),
                // A key a widget took — typed into an entry — is its own.
                iced::Event::Keyboard(event) if status == iced::event::Status::Ignored => {
                    shortcut(event)
                }
                _ => None,
            }),
        ])
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Window(action) => {
                return window::perform(action)
                    .chain(window::is_maximized().map(Message::Maximized));
            }
            Message::Focused(focused) => self.focused = focused,
            Message::Maximized(maximized) => self.maximized = maximized,
            Message::Resized => return window::is_maximized().map(Message::Maximized),
            Message::SystemScheme(mode) => {
                self.scheme = match mode {
                    Mode::Dark => ColorScheme::Dark,
                    Mode::None | Mode::Light => ColorScheme::Light,
                };
            }
            Message::ToastDismissed(id) => self.toasts.dismiss(id),

            Message::SectionSelected(index) => {
                self.show_content = true;
                return self.open_section(Section::ALL[index]);
            }
            Message::ContentShown(shown) => self.show_content = shown,
            Message::RootShown => self.switching = false,
            Message::Listed(tag, result) => {
                if let Some(folder) = self.folder_mut(&tag) {
                    folder.listing = match result {
                        Ok(entries) => {
                            // What was just made or uploaded is selected,
                            // unfiltered, so that it is among the rows.
                            let names = std::mem::take(&mut folder.select_next);
                            let selected: std::collections::BTreeSet<usize> = entries
                                .iter()
                                .enumerate()
                                .filter(|(_, entry)| names.contains(&entry.name))
                                .map(|(index, _)| index)
                                .collect();
                            let first = selected.first().copied();
                            if first.is_some() {
                                folder.filter.clear();
                            }
                            folder.selected = Selection::Multiple(selected);
                            folder.listing = Listing::Loaded(entries);

                            // And brought into sight, once the view has it.
                            return match first {
                                Some(index) => list_view::scroll_to(view_id(&tag), index),
                                None => Task::none(),
                            };
                        }
                        Err(drive::Error::AuthRequired) => {
                            self.signed_out = true;
                            return Task::none();
                        }
                        Err(error) => {
                            tracing::warn!(%error, "listing failed");
                            Listing::Failed(error.to_string())
                        }
                    };
                }
            }
            Message::Filtered(tag, filter) => {
                if let Some(folder) = self.folder_mut(&tag) {
                    folder.filter = filter;
                    folder.selected = Selection::Multiple(Default::default());
                }
            }
            Message::Reload(tag) => return self.load(&tag),
            Message::RefreshTop => {
                if let Some(tag) = self
                    .live_pages()
                    .last()
                    .filter(|page| matches!(page.kind, PageKind::Folder(_)))
                    .map(|page| page.tag.clone())
                {
                    return self.load(&tag);
                }
            }
            Message::Selected(tag, selection) => {
                if let Some(folder) = self.folder_mut(&tag) {
                    folder.selected = selection;
                }
            }
            Message::Activated(tag, index) => {
                let Some(uid) = self
                    .folder_mut(&tag)
                    .and_then(|folder| folder.shown().get(index).map(|entry| entry.uid.clone()))
                else {
                    return Task::none();
                };
                return self.activate(&tag, &uid);
            }
            Message::CancelOperation(id) => {
                let Some(at) = self.operations.iter().position(|op| op.id == id) else {
                    return Task::none();
                };
                // Dropping the operation aborts its task, and the CLI with it.
                let operation = self.operations.remove(at);
                tracing::info!(status = %operation.status, "cancelled");

                if operation.transfer
                    && let Some(folder) = self.folder_mut(&operation.tag)
                {
                    folder.transfers = folder.transfers.saturating_sub(1);
                }
                // Whatever was done before the kill shows.
                return self.load(&operation.tag);
            }
            Message::ShareIn(tag, uid) => {
                let Some(cli) = self.cli.clone() else {
                    return Task::none();
                };
                let Some(entry) = self
                    .folder_mut(&tag)
                    .and_then(|folder| folder.entry(&uid).cloned())
                else {
                    return Task::none();
                };

                let path = entry.path.clone();
                self.share = Some(Share {
                    entry,
                    sharing: None,
                    email: String::new(),
                    role: Role::Viewer,
                    message: String::new(),
                    include_name: false,
                    link_password: String::new(),
                    link_expiry: String::new(),
                    busy: None,
                });
                self.dialog = Some(Dialog::Share);
                self.dialog_open = true;

                return Task::perform(cli.sharing(path), move |result| {
                    Message::ShareLoaded(uid.clone(), result)
                });
            }
            Message::ShareLoaded(uid, result) => {
                if let Some(share) = &mut self.share
                    && share.entry.uid == uid
                {
                    share.busy = None;
                    match result {
                        Ok(sharing) => {
                            // The link's settings as they stand, to edit.
                            let link = sharing.as_ref().and_then(|sharing| sharing.link.as_ref());
                            share.link_password = link
                                .and_then(|link| link.password.clone())
                                .unwrap_or_default();
                            share.link_expiry = link
                                .and_then(|link| link.expires.as_deref())
                                .and_then(|expires| {
                                    chrono::DateTime::parse_from_rfc3339(expires).ok()
                                })
                                .map(|expires| {
                                    expires
                                        .with_timezone(&chrono::Local)
                                        .date_naive()
                                        .to_string()
                                })
                                .unwrap_or_default();
                            share.sharing = Some(Ok(sharing));
                            // The entry's "Shared" follows what was done.
                            let shared = matches!(&share.sharing, Some(Ok(Some(_))));
                            let uid = share.entry.uid.clone();
                            for page in &mut self.pages {
                                if let PageKind::Folder(Folder {
                                    listing: Listing::Loaded(entries),
                                    ..
                                }) = &mut page.kind
                                    && let Some(entry) =
                                        entries.iter_mut().find(|entry| entry.uid == uid)
                                {
                                    entry.shared = shared;
                                }
                            }
                        }
                        Err(drive::Error::AuthRequired) => self.signed_out = true,
                        Err(error) => {
                            tracing::warn!(%error, "sharing failed");
                            // A change that failed leaves what was known.
                            if share.sharing.is_none() {
                                share.sharing = Some(Err(error.to_string()));
                            } else {
                                self.toast(format!("Could not change the sharing: {error}"));
                            }
                        }
                    }
                }
            }
            Message::ShareEmail(email) => {
                if let Some(share) = &mut self.share {
                    share.email = email;
                }
            }
            Message::ShareRole(role) => {
                if let Some(share) = &mut self.share {
                    share.role = role;
                }
            }
            Message::InviteText(message) => {
                if let Some(share) = &mut self.share {
                    share.message = message;
                }
            }
            Message::ShareIncludeName(include) => {
                if let Some(share) = &mut self.share {
                    share.include_name = include;
                }
            }
            Message::LinkPassword(password) => {
                if let Some(share) = &mut self.share {
                    share.link_password = password;
                }
            }
            Message::LinkExpiry(expiry) => {
                if let Some(share) = &mut self.share {
                    share.link_expiry = expiry;
                }
            }
            Message::LinkApply => {
                let Some(cli) = self.cli.clone() else {
                    return Task::none();
                };
                let Some(share) = &mut self.share else {
                    return Task::none();
                };
                let (Some(link), Ok(expiry)) = (share.link(), share.expiry()) else {
                    return Task::none();
                };
                if share.busy.is_some() {
                    return Task::none();
                }
                let (role, password) = (link.role, Some(share.link_password.clone()));
                share.busy = Some(Busy::LinkSettings);
                let (uid, path) = (share.entry.uid.clone(), share.entry.path.clone());
                tracing::info!(
                    password = if share.link_password.is_empty() {
                        "none"
                    } else {
                        "set"
                    },
                    ?expiry,
                    "public link settings"
                );
                return Task::perform(cli.set_link(path, role, password, expiry), move |result| {
                    Message::ShareLoaded(uid.clone(), result)
                });
            }
            Message::Invite => {
                let Some(cli) = self.cli.clone() else {
                    return Task::none();
                };
                let Some(share) = &mut self.share else {
                    return Task::none();
                };
                let email = share.email.trim().to_owned();
                if share.busy.is_some() || !looks_like_email(&email) {
                    return Task::none();
                }
                share.busy = Some(Busy::Invite);
                share.email.clear();
                let (uid, path, role) = (
                    share.entry.uid.clone(),
                    share.entry.path.clone(),
                    share.role,
                );
                let message = Some(share.message.clone());
                let include_name = share.include_name;
                tracing::info!(%email, %role, "inviting");
                return Task::perform(
                    cli.invite(path, vec![email], role, message, include_name),
                    move |result| Message::ShareLoaded(uid.clone(), result),
                );
            }
            Message::Uninvite(email) => {
                let Some(cli) = self.cli.clone() else {
                    return Task::none();
                };
                let Some(share) = &mut self.share else {
                    return Task::none();
                };
                if share.busy.is_some() {
                    return Task::none();
                }
                share.busy = Some(Busy::Uninvite(email.clone()));
                let (uid, path) = (share.entry.uid.clone(), share.entry.path.clone());
                tracing::info!(%email, "removing from the share");
                return Task::perform(cli.unshare(path, vec![email]), move |result| {
                    Message::ShareLoaded(uid.clone(), result)
                });
            }
            Message::LinkToggled(on) => {
                let Some(cli) = self.cli.clone() else {
                    return Task::none();
                };
                let Some(share) = &mut self.share else {
                    return Task::none();
                };
                if share.busy.is_some() {
                    return Task::none();
                }
                share.busy = Some(Busy::Link);
                let (uid, path) = (share.entry.uid.clone(), share.entry.path.clone());
                tracing::info!(on, "public link");
                let change = async move {
                    if on {
                        cli.set_link(path, Role::Viewer, None, None).await
                    } else {
                        cli.remove_link(path).await
                    }
                };
                return Task::perform(change, move |result| {
                    Message::ShareLoaded(uid.clone(), result)
                });
            }
            Message::LinkRole(role) => {
                let Some(cli) = self.cli.clone() else {
                    return Task::none();
                };
                let Some(share) = &mut self.share else {
                    return Task::none();
                };
                if share.busy.is_some() {
                    return Task::none();
                }
                // The role goes with the password and expiry the link has.
                let (password, expiry) = match share.link() {
                    Some(link) => (link.password.clone(), link.expires.clone()),
                    None => (None, None),
                };
                share.busy = Some(Busy::LinkRole);
                let (uid, path) = (share.entry.uid.clone(), share.entry.path.clone());
                tracing::info!(%role, "public link role");
                return Task::perform(cli.set_link(path, role, password, expiry), move |result| {
                    Message::ShareLoaded(uid.clone(), result)
                });
            }
            Message::Leave(tag, uid) => {
                let Some(cli) = self.cli.clone() else {
                    return Task::none();
                };
                let Some(entry) = self
                    .folder_mut(&tag)
                    .and_then(|folder| folder.entry(&uid).cloned())
                else {
                    return Task::none();
                };
                let operation = self.begin(
                    format!("Leaving “{}”", entry.name),
                    "Shared with you".to_owned(),
                    &tag,
                    false,
                );
                tracing::info!(name = %entry.name, "leaving");
                let task = Task::perform(cli.leave(entry.path), {
                    let tag = tag.clone();
                    move |result| Message::Left(operation, tag.clone(), entry.name.clone(), result)
                });
                return self.track(operation, task);
            }
            Message::Left(operation, tag, name, result) => {
                self.finish(operation);
                match result {
                    Ok(()) => tracing::info!(%name, "left"),
                    Err(drive::Error::AuthRequired) => self.signed_out = true,
                    Err(error) => self.toast(format!("Could not leave “{name}”: {error}")),
                }
                return self.load(&tag);
            }
            Message::Invitations(tag, result) => {
                if let Some(folder) = self.folder_mut(&tag) {
                    match result {
                        Ok(invitations) => folder.invitations = invitations,
                        Err(drive::Error::AuthRequired) => self.signed_out = true,
                        Err(error) => tracing::warn!(%error, "no invitations"),
                    }
                }
            }
            Message::AcceptInvitation(tag, invitation)
            | Message::RejectInvitation(tag, invitation)
                if self.cli.is_none() =>
            {
                let _ = (tag, invitation);
            }
            Message::AcceptInvitation(tag, invitation) => {
                let cli = self.cli.clone().expect("checked above");
                let operation = self.begin(
                    format!("Accepting “{}”", invitation.name),
                    invitation
                        .from
                        .as_deref()
                        .map_or("Shared with you".to_owned(), |from| format!("From {from}")),
                    &tag,
                    false,
                );
                tracing::info!(name = %invitation.name, "accepting an invitation");
                let task = Task::perform(cli.accept_invitation(invitation.uid), {
                    let tag = tag.clone();
                    move |result| {
                        Message::InvitationAnswered(
                            operation,
                            tag.clone(),
                            invitation.name.clone(),
                            result,
                        )
                    }
                });
                return self.track(operation, task);
            }
            Message::RejectInvitation(tag, invitation) => {
                let cli = self.cli.clone().expect("checked above");
                let operation = self.begin(
                    format!("Declining “{}”", invitation.name),
                    invitation
                        .from
                        .as_deref()
                        .map_or("Shared with you".to_owned(), |from| format!("From {from}")),
                    &tag,
                    false,
                );
                tracing::info!(name = %invitation.name, "declining an invitation");
                let task = Task::perform(cli.reject_invitation(invitation.uid), {
                    let tag = tag.clone();
                    move |result| {
                        Message::InvitationAnswered(
                            operation,
                            tag.clone(),
                            invitation.name.clone(),
                            result,
                        )
                    }
                });
                return self.track(operation, task);
            }
            Message::InvitationAnswered(operation, tag, name, result) => {
                self.finish(operation);
                match result {
                    Ok(()) => tracing::info!(%name, "invitation answered"),
                    Err(drive::Error::AuthRequired) => self.signed_out = true,
                    Err(error) => self.toast(format!("Could not answer for “{name}”: {error}")),
                }
                return self.load(&tag);
            }
            Message::DeleteSelected => {
                let Some((tag, writable)) = self.top_folder() else {
                    return Task::none();
                };
                if self.section == Section::Trash {
                    return self.update(Message::ConfirmDelete(tag));
                }
                if writable {
                    return self.update(Message::Trash(tag));
                }
            }
            Message::Trash(tag) => {
                let Some(cli) = self.cli.clone() else {
                    return Task::none();
                };
                let Some((entries, from)) = self
                    .folder_mut(&tag)
                    .map(|folder| {
                        (
                            folder
                                .selected_entries()
                                .into_iter()
                                .cloned()
                                .collect::<Vec<_>>(),
                            folder.title.clone(),
                        )
                    })
                    .filter(|(entries, _)| !entries.is_empty())
                else {
                    return Task::none();
                };

                let operation = self.begin(
                    format!("Moving {} to Trash", describe(&entries)),
                    format!("From {from}"),
                    &tag,
                    false,
                );
                tracing::info!(count = entries.len(), "trashing");
                let task = Task::perform(cli.trash(entries.clone()), {
                    let tag = tag.clone();
                    move |result| Message::Trashed(operation, tag.clone(), entries.clone(), result)
                });
                return self.track(operation, task);
            }
            Message::Trashed(operation, tag, entries, result) => {
                self.finish(operation);
                match result {
                    Ok(done) if done.failed.is_empty() => {
                        tracing::info!(count = done.done, "trashed");
                        // Undone by restoring, as Files' toast offers.
                        self.toasts.add(
                            toast::toast(format!("{} moved to Trash", describe(&entries)))
                                .button("Undo", Message::Restore(tag.clone(), entries)),
                        );
                    }
                    Ok(done) => self.toast(failed("move", "to Trash", &done)),
                    Err(drive::Error::AuthRequired) => self.signed_out = true,
                    Err(error) => self.toast(format!("Could not move to Trash: {error}")),
                }
                return self.load(&tag);
            }
            Message::Restore(tag, entries) => {
                let Some(cli) = self.cli.clone() else {
                    return Task::none();
                };
                let operation = self.begin(
                    format!("Restoring {}", describe(&entries)),
                    "From Trash".to_owned(),
                    &tag,
                    false,
                );
                tracing::info!(count = entries.len(), "restoring");
                let task = Task::perform(cli.restore(entries.clone()), {
                    let tag = tag.clone();
                    move |result| Message::Restored(operation, tag.clone(), entries.clone(), result)
                });
                return self.track(operation, task);
            }
            Message::Restored(operation, tag, entries, result) => {
                self.finish(operation);
                match result {
                    Ok(done) if done.failed.is_empty() => {
                        tracing::info!(count = done.done, "restored");
                        if let Some(folder) = self.folder_mut(&tag) {
                            folder.select_next =
                                entries.into_iter().map(|entry| entry.name).collect();
                        }
                    }
                    Ok(done) => self.toast(failed("restore", "", &done)),
                    Err(drive::Error::AuthRequired) => self.signed_out = true,
                    Err(error) => self.toast(format!("Could not restore: {error}")),
                }
                return self.load(&tag);
            }
            Message::ConfirmDelete(tag) => {
                let entries: Vec<Entry> = self
                    .folder_mut(&tag)
                    .map(|folder| folder.selected_entries().into_iter().cloned().collect())
                    .unwrap_or_default();
                if entries.is_empty() {
                    return Task::none();
                }
                self.deleting = Some((tag, entries));
                self.dialog = Some(Dialog::Delete);
                self.dialog_open = true;
            }
            Message::DeleteForever => {
                let Some(cli) = self.cli.clone() else {
                    return Task::none();
                };
                let Some((tag, entries)) = self.deleting.take() else {
                    return Task::none();
                };
                self.dialog_open = false;

                let operation = self.begin(
                    format!("Deleting {}", describe(&entries)),
                    "From Trash, permanently".to_owned(),
                    &tag,
                    false,
                );
                tracing::info!(count = entries.len(), "deleting for good");
                let task = Task::perform(cli.delete(entries.clone()), {
                    let tag = tag.clone();
                    move |result| Message::Deleted(operation, tag.clone(), entries.clone(), result)
                });
                return self.track(operation, task);
            }
            Message::Deleted(operation, tag, _entries, result) => {
                self.finish(operation);
                match result {
                    Ok(done) if done.failed.is_empty() => {
                        tracing::info!(count = done.done, "deleted");
                    }
                    Ok(done) => self.toast(failed("delete", "", &done)),
                    Err(drive::Error::AuthRequired) => self.signed_out = true,
                    Err(error) => self.toast(format!("Could not delete: {error}")),
                }
                return self.load(&tag);
            }
            Message::Copy(tag) => self.take(&tag, false),
            Message::Cut(tag) => self.take(&tag, true),
            Message::CopySelected => {
                if self.dialog.is_none()
                    && let Some((tag, _)) = self.top_folder()
                {
                    self.take(&tag, false);
                }
            }
            Message::CutSelected => {
                if self.dialog.is_none()
                    && let Some((tag, _)) = self.top_folder()
                {
                    self.take(&tag, true);
                }
            }
            Message::Paste(tag) => {
                let Some((target, into)) = self
                    .folder_mut(&tag)
                    .map(|folder| (folder.path.clone(), folder.title.clone()))
                else {
                    return Task::none();
                };
                return self.paste(tag, target, into, true);
            }
            Message::PasteHere => {
                if self.dialog.is_none()
                    && let Some(tag) = self.writable_top()
                {
                    return self.update(Message::Paste(tag));
                }
            }
            Message::PasteInto(tag, uid) => {
                let Some((target, into)) = self.folder_mut(&tag).and_then(|folder| {
                    folder
                        .entry(&uid)
                        .map(|entry| (entry.path.clone(), entry.name.clone()))
                }) else {
                    return Task::none();
                };
                return self.paste(tag, target, into, false);
            }
            Message::Pasted(operation, tag, names, cut, result) => {
                self.finish(operation);
                let verb = if cut { "move" } else { "copy" };
                match result {
                    Ok(done) if done.failed.is_empty() => {
                        tracing::info!(count = done.done, cut, "pasted");
                        // What was moved is where it was pasted, and only
                        // there: Files' clipboard empties too.
                        if cut {
                            self.clipboard = None;
                        }
                        // The banner has had its paste.
                        if let Some(clipboard) = &mut self.clipboard {
                            clipboard.shown = false;
                        }
                        // A copy may have been named "(copy)" on the way.
                        let names = if done.names.is_empty() {
                            names
                        } else {
                            done.names
                        };
                        if let Some(folder) = self.folder_mut(&tag) {
                            folder.select_next = names;
                        }
                    }
                    Ok(done) => self.toast(failed(verb, "", &done)),
                    Err(drive::Error::AuthRequired) => self.signed_out = true,
                    Err(error) => self.toast(format!("Could not {verb}: {error}")),
                }
                return self.load(&tag);
            }
            Message::DismissTaken => {
                if let Some(clipboard) = &mut self.clipboard {
                    clipboard.shown = false;
                }
            }
            Message::SelectAll(tag) => {
                if let Some(folder) = self.folder_mut(&tag) {
                    let all = (0..folder.shown().len()).collect();
                    folder.selected = Selection::Multiple(all);
                }
            }
            Message::ConfirmEmptyTrash => {
                self.dialog = Some(Dialog::EmptyTrash);
                self.dialog_open = true;
            }
            Message::EmptyTrash => {
                let Some(cli) = self.cli.clone() else {
                    return Task::none();
                };
                self.dialog_open = false;

                let tag = self.top_folder().map(|(tag, _)| tag).unwrap_or_default();
                let operation = self.begin(
                    "Emptying Trash".to_owned(),
                    "Deleting everything in it permanently".to_owned(),
                    &tag,
                    false,
                );
                tracing::info!("emptying the trash");
                let task = Task::perform(cli.empty_trash(), move |result| {
                    Message::TrashEmptied(operation, result)
                });
                return self.track(operation, task);
            }
            Message::TrashEmptied(operation, result) => {
                self.finish(operation);
                match result {
                    Ok(()) => tracing::info!("trash emptied"),
                    Err(drive::Error::AuthRequired) => self.signed_out = true,
                    Err(error) => self.toast(format!("Could not empty the trash: {error}")),
                }
                // Everything in it is gone, so back to its root.
                if self.section == Section::Trash {
                    return self.open_section(Section::Trash);
                }
            }
            Message::Escape => {
                // A dialog takes its own Escape; so does an open menu.
                if self.dialog.is_some() {
                    return Task::none();
                }

                // Then the selection of the folder shown, as Files clears
                // it; and only with nothing selected does it go back.
                let selection = self.pages.iter_mut().rev().find(|page| !page.popped);
                if let Some(Page {
                    kind: PageKind::Folder(folder),
                    ..
                }) = selection
                    && folder.selected.iter().next().is_some()
                {
                    folder.selected = Selection::Multiple(Default::default());
                    return Task::none();
                }

                if self.live_pages().count() > 1 {
                    return self.update(Message::Back);
                }
            }
            Message::Download(tag) => {
                let Some(folder) = self.folder_mut(&tag) else {
                    return Task::none();
                };
                let paths: Vec<String> = folder
                    .selected_entries()
                    .into_iter()
                    .map(|entry| entry.path.clone())
                    .collect();
                if paths.is_empty() {
                    return Task::none();
                }

                let mut chooser = rfd::AsyncFileDialog::new().set_title("Download To");
                if let Some(downloads) = dirs::download_dir() {
                    chooser = chooser.set_directory(downloads);
                }
                return Task::perform(chooser.pick_folder(), move |dir| {
                    Message::DownloadTo(
                        tag.clone(),
                        paths.clone(),
                        dir.map(|dir| dir.path().to_owned()),
                    )
                });
            }
            Message::DownloadTo(tag, paths, Some(dir)) => return self.download(&tag, paths, dir),
            Message::DownloadTo(_, _, None) => {}
            Message::Downloaded(operation, tag, result) => {
                self.finish(operation);
                if let Some(folder) = self.folder_mut(&tag) {
                    folder.transfers = folder.transfers.saturating_sub(1);
                }
                match result {
                    Ok(transfer) => {
                        tracing::info!(
                            downloaded = transfer.transferred,
                            skipped = transfer.skipped,
                            "download done"
                        );
                        if let Some(failure) = transfer_failure("download", &transfer) {
                            self.toast(failure);
                        }
                    }
                    Err(drive::Error::AuthRequired) => self.signed_out = true,
                    Err(error) => self.toast(format!("Could not download: {error}")),
                }
            }
            Message::ShowInfo(tag, uid) => {
                let Some(cli) = self.cli.clone() else {
                    return Task::none();
                };
                let Some(folder) = self.folder_mut(&tag) else {
                    return Task::none();
                };
                let Some(entry) = folder.entry(&uid).cloned() else {
                    return Task::none();
                };

                let path = entry.path.clone();
                self.info = Some(Info {
                    entry,
                    location: folder.trail.clone(),
                    details: None,
                });
                self.dialog = Some(Dialog::Info);
                self.dialog_open = true;

                return Task::perform(cli.info(path), move |result| {
                    Message::InfoLoaded(uid.clone(), result)
                });
            }
            Message::InfoLoaded(uid, result) => {
                if let Some(info) = &mut self.info
                    && info.entry.uid == uid
                {
                    if let Err(error) = &result {
                        tracing::warn!(%error, "no info");
                    }
                    info.details = Some(result.map_err(|error| error.to_string()));
                }
            }
            Message::Opened(tag, uid, result) => return self.opened(&tag, &uid, result),
            Message::OpenExternally(target) => {
                return Task::perform(files::open_externally(target), Message::OpenedExternally);
            }
            Message::OpenedExternally(result) => {
                if let Err(error) = result {
                    self.toast(error);
                }
            }
            Message::Edited(tag, action) => {
                // Read-only: the cursor moves and selects, the text stays.
                if !action.is_edit()
                    && let Some(Page {
                        kind:
                            PageKind::Viewer(Viewer {
                                body: Body::Text(content),
                                ..
                            }),
                        ..
                    }) = self.page_mut(&tag)
                {
                    content.perform(action);
                }
            }
            Message::Back => {
                if self.live_pages().count() > 1 {
                    if let Some(page) = self.pages.iter_mut().rev().find(|page| !page.popped) {
                        page.popped = true;
                    }
                } else {
                    self.show_content = false;
                }
            }
            Message::PageHidden(tag) => self.pages.retain(|page| !(page.popped && page.tag == tag)),

            Message::SignIn => {
                if let Some(cli) = self.cli.clone() {
                    let (task, handle) = Task::run(cli.login(), Message::Login).abortable();
                    self.sign_in = Some(SignIn {
                        url: None,
                        _task: handle.abort_on_drop(),
                    });
                    return task;
                }
            }
            Message::Login(Login::Url(url)) => {
                if let Some(sign_in) = &mut self.sign_in {
                    sign_in.url = Some(url);
                }
            }
            Message::Login(Login::Done(result)) => {
                self.sign_in = None;
                match result {
                    Ok(()) => {
                        self.signed_out = false;
                        return self.open_section(self.section);
                    }
                    Err(error) => self.toast(format!("Could not sign in: {error}")),
                }
            }
            Message::CancelSignIn => self.sign_in = None,
            Message::CopyText(text, what) => {
                return iced::clipboard::write(text).map(move |_| Message::Copied(what));
            }
            Message::Copied(what) => {
                self.copied = Some(what);
                return Task::perform(
                    tokio::time::sleep(std::time::Duration::from_secs(1)),
                    move |_| Message::CopyShown(what),
                );
            }
            Message::CopyShown(what) => {
                if self.copied == Some(what) {
                    self.copied = None;
                }
            }
            Message::LogOut => {
                if let Some(cli) = self.cli.clone() {
                    return Task::perform(cli.logout(), Message::LoggedOut);
                }
            }
            Message::LoggedOut(result) => match result {
                Ok(()) | Err(drive::Error::AuthRequired) => {
                    tracing::info!("logged out");
                    self.signed_out = true;
                    self.account = None;
                    self.pages.clear();
                    self.dialog_open = false;
                    let _ = std::fs::remove_dir_all(files::cache_dir_root());
                }
                Err(error) => self.toast(format!("Could not log out: {error}")),
            },

            Message::ToggleView => {
                self.config.view = self.config.view.toggled();
                return self.save_settings();
            }
            Message::SettingsSaved(result) => {
                if let Err(error) = result {
                    self.toast(error);
                }
            }
            Message::ShowDialog(which) => {
                self.dialog = Some(which);
                self.dialog_open = true;
                self.about_pages.clear();
                if which == Dialog::Preferences {
                    return self.ask_about_cli();
                }
            }
            Message::CloseDialog => self.dialog_open = false,
            Message::DialogClosed => {
                self.dialog = None;
                self.naming = None;
                self.info = None;
                self.deleting = None;
                self.share = None;
            }
            Message::CliPathInput(path) => self.cli_path_input = path,
            Message::ApplyCliPath => return self.apply_cli_path(),
            Message::CliVersion(result) => {
                self.cli_version = Some(result.map_err(|error| error.to_string()));
            }
            Message::AccountLoaded(result) => match result {
                Err(drive::Error::AuthRequired) => self.account = None,
                result => self.account = Some(result.map_err(|error| error.to_string())),
            },
            Message::AboutPush(page) => self.about_pages.push(page),
            Message::AboutPop => {
                self.about_pages.pop();
            }

            Message::UploadHere => {
                if let Some(tag) = self.writable_top() {
                    return self.update(Message::UploadTo(tag));
                }
            }
            Message::UploadTo(tag) => {
                tracing::debug!(%tag, "choosing files to upload");
                return Task::perform(
                    rfd::AsyncFileDialog::new()
                        .set_title("Upload Files")
                        .pick_files(),
                    move |files| {
                        let files = files
                            .unwrap_or_default()
                            .into_iter()
                            .map(|file| file.path().to_owned())
                            .collect();
                        Message::Picked(tag.clone(), files)
                    },
                );
            }
            Message::Picked(tag, files) => return self.upload(&tag, files),
            Message::Uploaded(operation, tag, names, result) => {
                self.finish(operation);
                return self.uploaded(&tag, names, result);
            }
            Message::NewFolderHere => {
                if let Some(tag) = self.writable_top() {
                    return self.update(Message::NewFolderIn(tag));
                }
            }
            Message::NewFolderIn(tag) => {
                self.naming = Some(Naming {
                    tag,
                    name: String::new(),
                    busy: false,
                    rename: None,
                });
                self.dialog = Some(Dialog::NewFolder);
                self.dialog_open = true;
                return iced::widget::operation::focus(dialogs::NAME_ENTRY);
            }
            Message::RenameSelected => {
                let Some(tag) = self.writable_top() else {
                    return Task::none();
                };
                let selected = self
                    .folder_mut(&tag)
                    .map(|folder| {
                        folder
                            .selected_entries()
                            .iter()
                            .map(|entry| entry.uid.clone())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if let [uid] = selected.as_slice() {
                    return self.update(Message::RenameIn(tag, uid.clone()));
                }
            }
            Message::RenameIn(tag, uid) => {
                let Some(entry) = self
                    .folder_mut(&tag)
                    .and_then(|folder| folder.entry(&uid).cloned())
                else {
                    return Task::none();
                };

                // The name is offered with its stem selected, as Files
                // offers it: a new name, typed over, keeps the extension.
                let stem = match (entry.is_folder(), entry.name.rsplit_once('.')) {
                    (false, Some((stem, extension)))
                        if !stem.is_empty() && !extension.is_empty() =>
                    {
                        stem.chars().count()
                    }
                    _ => entry.name.chars().count(),
                };
                self.naming = Some(Naming {
                    tag,
                    name: entry.name.clone(),
                    busy: false,
                    rename: Some(entry),
                });
                self.dialog = Some(Dialog::NewFolder);
                self.dialog_open = true;

                let at = |index| iced::advanced::text::Position { line: 0, index };
                return iced::widget::operation::focus(dialogs::NAME_ENTRY).chain(
                    iced::widget::operation::select_range(dialogs::NAME_ENTRY, at(0), at(stem)),
                );
            }
            Message::NameTyped(name) => {
                if let Some(naming) = &mut self.naming {
                    naming.name = name;
                }
            }
            Message::NameSubmitted => return self.submit_name(),
            Message::Renamed(operation, tag, old, new, result) => match result {
                Ok(()) => {
                    self.finish(operation);
                    tracing::info!(from = %old, to = %new, "renamed");
                    if let Some(folder) = self.folder_mut(&tag) {
                        folder.select_next = vec![new];
                    }
                    return self.load(&tag);
                }
                Err(drive::Error::AuthRequired) => {
                    self.finish(operation);
                    self.signed_out = true;
                }
                Err(error) => {
                    self.finish(operation);
                    self.toast(format!("Could not rename “{old}”: {error}"));
                }
            },
            Message::FolderCreated(operation, tag, name, result) => match result {
                Ok(()) => {
                    self.finish(operation);
                    tracing::info!(%name, "folder created");
                    if let Some(folder) = self.folder_mut(&tag) {
                        folder.select_next = vec![name];
                    }
                    return self.load(&tag);
                }
                Err(drive::Error::AuthRequired) => {
                    self.finish(operation);
                    self.signed_out = true;
                }
                Err(error) => {
                    self.finish(operation);
                    self.toast(format!("Could not create “{name}”: {error}"));
                }
            },
        }

        Task::none()
    }

    pub fn view(&self) -> Element<'_, Message> {
        let content = match &self.cli {
            None => self.missing_cli(),
            Some(_) if self.signed_out => self.signed_out_page(),
            Some(_) => breakpoint_bin(
                [breakpoint_bin::breakpoint(
                    breakpoint_bin::Condition::max_width(550.0, breakpoint_bin::Unit::Sp),
                    (),
                )],
                |narrow| self.shell(narrow.is_some()),
            )
            .boxed(),
        };

        let content = toast_overlay(content, &self.toasts).on_dismiss(Message::ToastDismissed);

        let content = dialog(content, self.dialog.map(|which| self.dialog_view(which)))
            .open(self.dialog_open)
            .on_close(Message::CloseDialog)
            .on_closed(Message::DialogClosed);

        adw::window(content)
            .on_window(Message::Window)
            .maximized(self.maximized)
            .rounded(ROUNDED_WINDOW)
            .boxed()
    }

    // ------------------------------------------------------------ navigation

    /// Replaces the stack with `section`'s root, and lists it.
    fn open_section(&mut self, section: Section) -> Task<Message> {
        self.section = section;
        self.pages.clear();
        self.switching = true;

        let tag = self.push(PageKind::Folder(Folder {
            title: section.title().into(),
            trail: section.title().into(),
            path: section.path().into(),
            listing: Listing::Loading,
            filter: String::new(),
            opening: None,
            writable: section == Section::MyFiles,
            transfers: 0,
            select_next: Vec::new(),
            invitations: Vec::new(),
            selected: Selection::Multiple(Default::default()),
        }));

        self.load(&tag)
    }

    fn push(&mut self, kind: PageKind) -> String {
        let tag = format!("page-{}", self.next_tag);
        self.next_tag += 1;
        self.pages.push(Page {
            tag: tag.clone(),
            popped: false,
            kind,
        });
        tag
    }

    /// Lists the folder `tag` afresh.
    fn load(&mut self, tag: &str) -> Task<Message> {
        let Some(cli) = self.cli.clone() else {
            return Task::none();
        };
        let Some(folder) = self.folder_mut(tag) else {
            return Task::none();
        };

        folder.listing = Listing::Loading;
        let path = folder.path.clone();
        let tag = tag.to_owned();

        let listing = Task::perform(cli.clone().list(path.clone()), {
            let tag = tag.clone();
            move |result| Message::Listed(tag.clone(), result)
        });
        if path != Section::SharedWithMe.path() {
            return listing;
        }

        // The invitations waiting go above what is shared already.
        let invitations = Task::perform(cli.invitations(), move |result| {
            Message::Invitations(tag.clone(), result)
        });
        Task::batch([listing, invitations])
    }

    /// A row was activated: a folder is pushed, a file downloaded to open.
    fn activate(&mut self, tag: &str, uid: &str) -> Task<Message> {
        let Some(cli) = self.cli.clone() else {
            return Task::none();
        };
        let Some(folder) = self.folder_mut(tag) else {
            return Task::none();
        };
        let Listing::Loaded(entries) = &folder.listing else {
            return Task::none();
        };
        let Some(entry) = entries.iter().find(|entry| entry.uid == uid).cloned() else {
            return Task::none();
        };
        if entry.is_folder() {
            let child = Folder {
                trail: format!("{} / {}", folder.trail, entry.name),
                title: entry.name,
                path: entry.path,
                listing: Listing::Loading,
                filter: String::new(),
                opening: None,
                // Anything below a section's root is a folder, but what is
                // in the trash stays as it is.
                writable: self.section != Section::Trash,
                transfers: 0,
                select_next: Vec::new(),
                invitations: Vec::new(),
                selected: Selection::Multiple(Default::default()),
            };
            let child = self.push(PageKind::Folder(child));
            return self.load(&child);
        }

        // A Proton Docs document or Sheets spreadsheet has no file to
        // download: it opens on docs.proton.me, as the web client opens it.
        if let Some(url) = files::document_url(&entry) {
            tracing::info!(name = %entry.name, "opening a document in the browser");
            return Task::perform(files::open_externally(url), Message::OpenedExternally);
        }

        folder.opening = Some(entry.uid.clone());
        let (tag, uid) = (tag.to_owned(), entry.uid.clone());

        Task::perform(open(cli, entry), move |result| {
            Message::Opened(tag.clone(), uid.clone(), result)
        })
    }

    /// A file has been downloaded: shown on a page of its own, or handed on.
    fn opened(
        &mut self,
        tag: &str,
        uid: &str,
        result: Result<Opened, drive::Error>,
    ) -> Task<Message> {
        let Some(folder) = self.folder_mut(tag) else {
            return Task::none();
        };
        // Only the latest file asked for opens.
        if folder.opening.as_deref() != Some(uid) {
            return Task::none();
        }
        folder.opening = None;

        let Listing::Loaded(entries) = &folder.listing else {
            return Task::none();
        };
        let Some(entry) = entries.iter().find(|entry| entry.uid == uid).cloned() else {
            return Task::none();
        };

        let opened = match result {
            Ok(opened) => opened,
            Err(drive::Error::AuthRequired) => {
                self.signed_out = true;
                return Task::none();
            }
            Err(error) => {
                self.toast(format!("Could not open “{}”: {error}", entry.name));
                return Task::none();
            }
        };

        let body = match opened.content {
            Content::Text(text) => Body::Text(text_editor::Content::with_text(&text)),
            Content::Image => Body::Image(image::Handle::from_path(&opened.path)),
            Content::Svg => Body::Svg(svg::Handle::from_path(&opened.path)),
            Content::External => {
                let target = opened.path.to_string_lossy().into_owned();
                return Task::perform(files::open_externally(target), Message::OpenedExternally);
            }
        };

        // Opened from a page no longer on top: the answer came too late.
        if self.live_pages().last().map(|page| page.tag.as_str()) != Some(tag) {
            return Task::none();
        }

        self.push(PageKind::Viewer(Viewer {
            entry,
            file: opened.path,
            body,
        }));
        Task::none()
    }

    fn live_pages(&self) -> impl DoubleEndedIterator<Item = &Page> {
        self.pages.iter().filter(|page| !page.popped)
    }

    fn page_mut(&mut self, tag: &str) -> Option<&mut Page> {
        self.pages.iter_mut().find(|page| page.tag == tag)
    }

    fn folder_mut(&mut self, tag: &str) -> Option<&mut Folder> {
        match self.page_mut(tag) {
            Some(Page {
                kind: PageKind::Folder(folder),
                popped: false,
                ..
            }) => Some(folder),
            _ => None,
        }
    }

    /// Registers an operation under way in the folder at `tag`, for the
    /// indicator; `track` then hands it its task.
    fn begin(&mut self, status: String, details: String, tag: &str, transfer: bool) -> u64 {
        let id = self.next_operation;
        self.next_operation += 1;
        self.operations.push(Operation {
            id,
            status,
            details,
            tag: tag.to_owned(),
            transfer,
            handle: None,
        });
        id
    }

    /// Makes the operation's task one its row can cancel.
    fn track(&mut self, id: u64, task: Task<Message>) -> Task<Message> {
        let (task, handle) = task.abortable();
        if let Some(operation) = self.operations.iter_mut().find(|op| op.id == id) {
            operation.handle = Some(handle.abort_on_drop());
        }
        task
    }

    fn finish(&mut self, id: u64) {
        self.operations.retain(|operation| operation.id != id);
    }

    /// A copy button's icon: the copy icon, cross-faded to a check mark
    /// for the second after it copied `what`, and back.
    pub(super) fn copy_icon(&self, what: &'static str) -> impl Widget<Message> + 'static {
        adw::view_stack(usize::from(self.copied == Some(what)))
            .push(icon(icons::edit_copy()))
            .push(icon(icons::object_select()))
            .enable_transitions(true)
    }

    fn toast(&mut self, title: String) {
        tracing::info!(toast = %title);
        self.toasts.add(toast::toast(title));
    }

    fn download(&mut self, tag: &str, paths: Vec<String>, dir: PathBuf) -> Task<Message> {
        let Some(cli) = self.cli.clone() else {
            return Task::none();
        };
        let Some(folder) = self.folder_mut(tag) else {
            return Task::none();
        };

        // The spinner in the header bar shows it is under way.
        folder.transfers += 1;
        tracing::info!(?paths, dir = %dir.display(), "downloading");

        let short = match paths.len() {
            1 => "Downloading 1 item".to_owned(),
            count => format!("Downloading {count} items"),
        };
        let operation = self.begin(short, format!("To {}", dir.display()), tag, true);

        let tag = tag.to_owned();
        let task = Task::perform(cli.download_to(paths, dir), move |result| {
            Message::Downloaded(operation, tag.clone(), result)
        });
        self.track(operation, task)
    }

    /// Whether the page at `tag` lists a section itself, not a folder in it.
    fn is_section_root(&self, tag: &str) -> bool {
        self.pages.iter().any(|page| {
            page.tag == tag
                && matches!(&page.kind, PageKind::Folder(folder) if folder.path == self.section.path())
        })
    }

    /// The folder on top of the stack, and whether things can go in it.
    fn top_folder(&self) -> Option<(String, bool)> {
        self.live_pages().last().and_then(|page| match &page.kind {
            PageKind::Folder(folder) => Some((page.tag.clone(), folder.writable)),
            _ => None,
        })
    }

    /// The folder on top of the stack, if files and folders can go in it.
    fn writable_top(&self) -> Option<String> {
        self.live_pages()
            .last()
            .filter(|page| matches!(&page.kind, PageKind::Folder(folder) if folder.writable))
            .map(|page| page.tag.clone())
    }

    /// Copy or Cut: the selection on the page goes to the clipboard, for
    /// Paste to copy or to move.
    fn take(&mut self, tag: &str, cut: bool) {
        // The trash's items are restored, not copied: the CLI copies from
        // My Files, Computers and Shared with Me only.
        if self.section == Section::Trash {
            return;
        }
        let Some((entries, from)) = self
            .folder_mut(tag)
            .map(|folder| {
                (
                    folder
                        .selected_entries()
                        .into_iter()
                        .cloned()
                        .collect::<Vec<_>>(),
                    folder.title.clone(),
                )
            })
            .filter(|(entries, _)| !entries.is_empty())
        else {
            return;
        };

        tracing::info!(count = entries.len(), cut, "taken for pasting");
        self.clipboard = Some(Clipboard {
            entries,
            cut,
            from,
            shown: true,
        });
    }

    /// Paste: the clipboard's items are copied, or moved, into the folder
    /// at `target`, named `into`, and the page `tag` is listed again —
    /// with them selected, when `select` says they will be among its rows.
    fn paste(&mut self, tag: String, target: String, into: String, select: bool) -> Task<Message> {
        let Some(cli) = self.cli.clone() else {
            return Task::none();
        };
        let Some(Clipboard {
            entries, cut, from, ..
        }) = self.clipboard.clone()
        else {
            return Task::none();
        };
        let names: Vec<String> = if select {
            entries.iter().map(|entry| entry.name.clone()).collect()
        } else {
            Vec::new()
        };

        let verb = if cut { "Moving" } else { "Copying" };
        let operation = self.begin(
            format!("{verb} {}", describe(&entries)),
            format!("From {from} to {into}"),
            &tag,
            false,
        );
        tracing::info!(count = entries.len(), cut, "pasting");
        let future = async move {
            if cut {
                cli.move_to(entries, target).await
            } else {
                cli.copy(entries, target).await
            }
        };
        let task = Task::perform(future, move |result| {
            Message::Pasted(operation, tag.clone(), names.clone(), cut, result)
        });
        self.track(operation, task)
    }

    fn upload(&mut self, tag: &str, files: Vec<PathBuf>) -> Task<Message> {
        let Some(cli) = self.cli.clone() else {
            return Task::none();
        };
        if files.is_empty() {
            return Task::none();
        }
        let Some(folder) = self.folder_mut(tag) else {
            return Task::none();
        };

        folder.transfers += 1;
        let parent = folder.path.clone();
        let names: Vec<String> = files
            .iter()
            .map(|file| {
                file.file_name()
                    .unwrap_or(file.as_os_str())
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        tracing::info!(files = ?files, %parent, "uploading");

        let into = folder.title.clone();
        let short = match names.as_slice() {
            [name] => format!("Uploading “{name}”"),
            names => format!("Uploading {} files", names.len()),
        };
        let operation = self.begin(short, format!("To {into}"), tag, true);

        // The spinner in the header bar shows it is under way.
        let tag = tag.to_owned();
        let task = Task::perform(cli.upload(files, parent), move |result| {
            Message::Uploaded(operation, tag.clone(), names.clone(), result)
        });
        self.track(operation, task)
    }

    fn uploaded(
        &mut self,
        tag: &str,
        names: Vec<String>,
        result: Result<Transfer, drive::Error>,
    ) -> Task<Message> {
        if let Some(folder) = self.folder_mut(tag) {
            folder.transfers = folder.transfers.saturating_sub(1);
            // What arrived, or was already there, is selected; only what
            // failed is told.
            folder.select_next = names;
        }

        let upload = match result {
            Ok(upload) => upload,
            Err(drive::Error::AuthRequired) => {
                self.signed_out = true;
                return Task::none();
            }
            Err(error) => {
                self.toast(format!("Could not upload: {error}"));
                return self.load(tag);
            }
        };

        for (name, reason) in &upload.failures {
            tracing::warn!(%name, reason = reason.as_deref().unwrap_or("unknown"), "upload failed");
        }
        tracing::info!(
            uploaded = upload.transferred,
            skipped = upload.skipped,
            "upload done"
        );
        if let Some(failure) = transfer_failure("upload", &upload) {
            self.toast(failure);
        }

        // The listing shows what arrived, or what was already there.
        self.load(tag)
    }

    /// Takes the name in the naming dialog: a folder created, or the item
    /// renamed.
    fn submit_name(&mut self) -> Task<Message> {
        let Some(cli) = self.cli.clone() else {
            return Task::none();
        };
        let Some(naming) = &self.naming else {
            return Task::none();
        };
        if naming.busy || !self.name_problem().is_ok() {
            return Task::none();
        }

        let tag = naming.tag.clone();
        let name = naming.name.trim().to_owned();
        let rename = naming.rename.clone();
        let Some((parent, into)) = self
            .folder_mut(&tag)
            .map(|folder| (folder.path.clone(), folder.title.clone()))
        else {
            return Task::none();
        };

        if let Some(naming) = &mut self.naming {
            naming.busy = true;
        }
        self.dialog_open = false;

        match rename {
            Some(entry) => {
                tracing::info!(from = %entry.name, to = %name, "renaming");
                let old = entry.name;
                let operation = self.begin(
                    format!("Renaming “{old}” to “{name}”"),
                    format!("In {into}"),
                    &tag,
                    false,
                );
                let task = Task::perform(cli.rename(entry.path, name.clone()), move |result| {
                    Message::Renamed(operation, tag.clone(), old.clone(), name.clone(), result)
                });
                self.track(operation, task)
            }
            None => {
                tracing::info!(%name, %parent, "creating a folder");
                let operation = self.begin(
                    format!("Creating folder “{name}”"),
                    format!("In {into}"),
                    &tag,
                    false,
                );
                let task = Task::perform(cli.create_folder(parent, name.clone()), move |result| {
                    Message::FolderCreated(operation, tag.clone(), name.clone(), result)
                });
                self.track(operation, task)
            }
        }
    }

    /// What is wrong with the name in the naming dialog: `Err(None)` while
    /// there is no name yet, or it is the item's own, `Err(Some(why))` for
    /// one that cannot be, as Files words it.
    fn name_problem(&self) -> Result<(), Option<String>> {
        let Some(naming) = &self.naming else {
            return Err(None);
        };
        let name = naming.name.trim();
        let kind = naming.kind();

        if name.is_empty() {
            return Err(None);
        }
        if naming
            .rename
            .as_ref()
            .is_some_and(|entry| entry.name == name)
        {
            return Err(None);
        }
        if name.contains('/') {
            return Err(Some(format!("{kind} names cannot contain “/”.")));
        }
        if name == "." || name == ".." {
            return Err(Some(format!(
                "A {} cannot be called “{name}”.",
                kind.to_lowercase()
            )));
        }
        if name.len() > 255 {
            return Err(Some(format!("{kind} name is too long.")));
        }

        let siblings = self.pages.iter().find_map(|page| match &page.kind {
            PageKind::Folder(Folder {
                listing: Listing::Loaded(entries),
                ..
            }) if page.tag == naming.tag => Some(entries),
            _ => None,
        });
        let taken = siblings.and_then(|entries| {
            entries
                .iter()
                .filter(|entry| {
                    naming
                        .rename
                        .as_ref()
                        .is_none_or(|own| own.uid != entry.uid)
                })
                .find(|entry| entry.name == name)
        });
        match taken {
            Some(entry) if entry.is_folder() => {
                Err(Some("A folder with that name already exists.".into()))
            }
            Some(_) => Err(Some("A file with that name already exists.".into())),
            None => Ok(()),
        }
    }

    fn save_settings(&self) -> Task<Message> {
        Task::perform(self.config.clone().save(), Message::SettingsSaved)
    }

    /// Asks the CLI for its version and the account, for the preferences.
    fn ask_about_cli(&mut self) -> Task<Message> {
        self.cli_version = None;
        self.account = None;

        let Some(cli) = self.cli.clone() else {
            return Task::none();
        };
        let version = Task::perform(cli.clone().version(), Message::CliVersion);
        if self.signed_out {
            return version;
        }

        Task::batch([
            version,
            Task::perform(cli.account(), Message::AccountLoaded),
        ])
    }

    /// Takes the CLI path from the preferences: saved, and the CLI looked
    /// for again.
    fn apply_cli_path(&mut self) -> Task<Message> {
        let input = self.cli_path_input.trim();
        let path = (!input.is_empty()).then(|| PathBuf::from(input));

        if let Some(path) = &path
            && !path.is_file()
        {
            self.toast(format!("There is no file at {}", path.display()));
            return Task::none();
        }

        self.config.cli_path = path;
        self.cli = Cli::locate(self.config.cli_path.as_deref());
        self.signed_out = false;
        self.sign_in = None;

        Task::batch([
            self.save_settings(),
            self.ask_about_cli(),
            self.open_section(self.section),
        ])
    }

    // ------------------------------------------------------------------ view

    fn shell(&self, collapsed: bool) -> Element<'_, Message> {
        let backdrop = !self.focused;

        let selected = Section::ALL
            .iter()
            .position(|section| *section == self.section);
        let places = sidebar::sidebar(selected)
            .push(
                sidebar::section()
                    .push(sidebar::item(Section::MyFiles.title()).icon(Section::MyFiles.icon()))
                    .push(sidebar::item(Section::Devices.title()).icon(Section::Devices.icon())),
            )
            .push(
                sidebar::section()
                    .title("Sharing")
                    .push(
                        sidebar::item(Section::SharedWithMe.title())
                            .icon(Section::SharedWithMe.icon()),
                    )
                    .push(
                        sidebar::item(Section::SharedByMe.title()).icon(Section::SharedByMe.icon()),
                    ),
            )
            .push(
                sidebar::section()
                    .push(sidebar::item(Section::Trash.title()).icon(Section::Trash.icon())),
            )
            .on_select(Message::SectionSelected)
            .mode(if collapsed {
                SidebarMode::Page
            } else {
                SidebarMode::Sidebar
            });

        let side = toolbar_view(places)
            .top(
                header_bar()
                    .title(window_title("Proton Drive").backdrop(backdrop))
                    .end(self.main_menu())
                    .show_end_title_buttons(collapsed)
                    .on_window(Message::Window)
                    .maximized(self.maximized)
                    .backdrop(backdrop),
            )
            .backdrop(backdrop);

        // Going to another section is not navigation: as Files changes
        // the location in place, the new root is there at once, and only
        // what is opened from it slides in.
        let stack = navigation_view()
            .animate_transitions(!self.switching)
            .extend(
                self.pages
                    .iter()
                    .enumerate()
                    .map(|(depth, page)| self.page(page, depth, collapsed)),
            )
            .stack(
                self.live_pages()
                    .map(|page| page.tag.clone())
                    .collect::<Vec<_>>(),
            )
            .on_pop(Message::Back)
            // Escape goes back only once there is no selection to clear,
            // which `Message::Escape` decides.
            .pop_on_escape(false);

        navigation_split_view(side, stack)
            .collapsed(collapsed)
            .show_content(self.show_content)
            .on_show_content(Message::ContentShown)
            .backdrop(backdrop)
            .boxed()
    }

    /// The operations indicator, after Files' `NautilusProgressIndicator`:
    /// a flat button with a spinner while something runs, beside the `+`,
    /// opening a popover of the operations, each as Files'
    /// `NautilusProgressInfoWidget` lays it: the status over a pulsing bar
    /// over the details, a circular cancel button beside them. `None`
    /// while nothing runs.
    fn operations_indicator(&self) -> Option<Element<'_, Message>> {
        use libadwaita_iced::widget::{popover_button, progress_bar};

        if self.operations.is_empty() {
            return None;
        }

        let rows = self.operations.iter().map(|operation| {
            let text = column![
                typography::label(operation.status.as_str())
                    .wrapping(text::Wrapping::None)
                    .ellipsis(text::Ellipsis::Middle),
                container(progress_bar::pulsing()).padding([0.0, 2.0]),
                typography::caption(operation.details.as_str()).style(adw::text::dimmed),
            ]
            .spacing(6)
            .width(300);

            row![
                text,
                container(
                    adw::circular_button(icons::process_stop())
                        .on_press(Message::CancelOperation(operation.id)),
                )
                .padding(iced::padding::left(20.0)),
            ]
            .align_y(Alignment::Center)
            .boxed()
        });
        let list = iced::widget::Column::with_children(rows)
            .spacing(12)
            .padding(6);

        // Files caps the list at 270px and scrolls past that.
        let popover: Element<'_, Message> = if self.operations.len() > 3 {
            adw::scrollable(list).height(270).boxed()
        } else {
            list.boxed()
        };

        Some(
            popover_button(spinner())
                .style(adw::button::flat)
                .image_button()
                .popover(popover)
                .boxed(),
        )
    }

    fn main_menu(&self) -> Element<'_, Message> {
        menu_button(icon(icons::open_menu()))
            .style(adw::button::flat)
            .image_button()
            .push(
                item("Refresh")
                    .accelerator("F5")
                    .on_activate(Message::RefreshTop),
            )
            .push(separator())
            .push(
                item("Preferences")
                    .accelerator("Ctrl+,")
                    .on_activate(Message::ShowDialog(Dialog::Preferences)),
            )
            .push(
                item("Keyboard Shortcuts")
                    .accelerator("Ctrl+?")
                    .on_activate(Message::ShowDialog(Dialog::Shortcuts)),
            )
            .push(item("About Cold Drive").on_activate(Message::ShowDialog(Dialog::About)))
            .boxed()
    }

    /// The header bar every content page has: the back button below the
    /// root, or back to the sidebar while collapsed.
    fn content_bar<'a>(
        &self,
        title: impl Widget<Message> + 'a,
        depth: usize,
        collapsed: bool,
    ) -> adw::header_bar::HeaderBar<'a, Message> {
        let backdrop = !self.focused;
        let bar = header_bar()
            .title(title)
            .show_start_title_buttons(collapsed)
            .on_window(Message::Window)
            .maximized(self.maximized)
            .backdrop(backdrop);

        if depth > 0 {
            bar.on_back(Message::Back)
        } else if collapsed {
            bar.on_back(Message::ContentShown(false))
        } else {
            bar
        }
    }

    fn page<'a>(
        &'a self,
        page: &'a Page,
        depth: usize,
        collapsed: bool,
    ) -> NavigationPage<'a, Message> {
        let backdrop = !self.focused;

        let (title, view) = match &page.kind {
            PageKind::Folder(folder) => {
                let parent = folder.trail.rsplit_once(" / ").map(|(parent, _)| parent);
                let toggle = match self.config.view {
                    View::List => icons::view_grid(),
                    View::Grid => icons::view_list(),
                };
                // While an upload runs, the spinner stands where Refresh
                // would: the listing is refreshed once it is done.
                let refresh: Element<'_, Message> = if folder.transfers > 0 {
                    container(spinner())
                        .width(metrics::BUTTON_IMAGE_MIN_SIZE.width)
                        .height(metrics::BUTTON_IMAGE_MIN_SIZE.height)
                        .align_x(Alignment::Center)
                        .align_y(Alignment::Center)
                        .boxed()
                } else {
                    adw::icon_button(icons::view_refresh())
                        .style(adw::button::flat)
                        .on_press(Message::Reload(page.tag.clone()))
                        .boxed()
                };

                let mut bar = self
                    .content_bar(
                        page_title(&folder.title, parent, backdrop),
                        depth,
                        collapsed,
                    )
                    .end(row![
                        adw::icon_button(toggle)
                            .style(adw::button::flat)
                            .on_press(Message::ToggleView),
                        refresh,
                    ]);
                if folder.writable {
                    bar = bar.start(add_menu(&page.tag));
                }
                if let Some(indicator) = self.operations_indicator() {
                    bar = bar.start(indicator);
                }

                let view = self.folder(&page.tag, folder);
                let view: Element<'_, Message> = if folder.writable {
                    // Files' background menu: a secondary press on the view
                    // where no row is — a row takes the press for its own
                    // menu first.
                    let paste = self
                        .clipboard
                        .as_ref()
                        .map(|_| Message::Paste(page.tag.clone()));
                    // And over it, once something is taken and until it
                    // is pasted, a banner saying so with Paste Here — the
                    // way to paste without a key or a menu — and a close.
                    let paste_bar = taken_banner(
                        self.clipboard.as_ref().filter(|clipboard| clipboard.shown),
                        Message::Paste(page.tag.clone()),
                        backdrop,
                    );
                    let menu = adw::context_menu(container(view).width(Fill).height(Fill))
                        .push(
                            item("New Folder…")
                                .accelerator("Ctrl+N")
                                .on_activate(Message::NewFolderIn(page.tag.clone())),
                        )
                        .push(
                            item("Upload Files…")
                                .accelerator("Ctrl+U")
                                .on_activate(Message::UploadTo(page.tag.clone())),
                        )
                        .push(separator())
                        .push(item("Paste").accelerator("Ctrl+V").on_activate_maybe(paste))
                        .push(separator())
                        .push(
                            item("Select All")
                                .accelerator("Ctrl+A")
                                .on_activate(Message::SelectAll(page.tag.clone())),
                        )
                        .boxed();
                    column![paste_bar, menu].boxed()
                } else {
                    view
                };
                let view: Element<'_, Message> = if self.section == Section::Trash {
                    // Files' trash bar: a banner over the view with the
                    // way to empty it. With nothing listed there is
                    // nothing to empty, and the banner slides away.
                    let full =
                        matches!(&folder.listing, Listing::Loaded(entries) if !entries.is_empty());
                    let trash_bar =
                        adw::banner("Items in the Trash still count toward your storage")
                            .button("Empty Trash…", Message::ConfirmEmptyTrash)
                            .surface(Surface::View)
                            .backdrop(backdrop)
                            .revealed(full)
                            .boxed();
                    column![trash_bar, view].boxed()
                } else {
                    view
                };

                (
                    folder.title.as_str(),
                    // The whole pane is the view surface, bar included, as
                    // Files' window carries `.view` under a flat bar: the
                    // list paints `--view-bg-color` under its rows, and
                    // nothing comes between the bar and them.
                    on_view(toolbar_view(view).top(bar).backdrop(backdrop)),
                )
            }
            PageKind::Viewer(viewer) => {
                let mut subtitle = Vec::new();
                if let Some(size) = viewer.entry.size {
                    subtitle.push(format::size(size));
                }
                if let Some(date) = viewer.entry.modified.as_deref().and_then(format::date) {
                    subtitle.push(date);
                }

                let subtitle = subtitle.join(" · ");
                let mut bar = self
                    .content_bar(
                        page_title(&viewer.entry.name, Some(&subtitle), backdrop),
                        depth,
                        collapsed,
                    )
                    .end(
                        adw::icon_button(icons::adw_external_link())
                            .style(adw::button::flat)
                            .on_press(Message::OpenExternally(
                                viewer.file.to_string_lossy().into_owned(),
                            )),
                    );
                if let Some(indicator) = self.operations_indicator() {
                    bar = bar.start(indicator);
                }

                (
                    viewer.entry.name.as_str(),
                    // Text and images are shown the same way.
                    on_view(
                        toolbar_view(self.viewer(&page.tag, viewer))
                            .top(bar)
                            .backdrop(backdrop),
                    ),
                )
            }
        };

        let mut page = navigation_page(page.tag.clone(), view)
            .title(title)
            .on_hidden(Message::PageHidden(page.tag.clone()));
        if depth == 0 {
            page = page.on_shown(Message::RootShown);
        }
        page
    }

    fn folder<'a>(&'a self, tag: &'a str, folder: &'a Folder) -> Element<'a, Message> {
        let entries = match &folder.listing {
            Listing::Loading => return center(spinner().size(32)).boxed(),
            Listing::Failed(error) => {
                return status_page()
                    .icon(icons::network_offline())
                    .title("Could Not Load Folder")
                    .description(error.as_str())
                    .child(
                        adw::pill_button("Retry")
                            .style(adw::button::suggested_pill)
                            .on_press(Message::Reload(tag.to_owned())),
                    )
                    .boxed();
            }
            Listing::Loaded(entries) if entries.is_empty() => {
                let (title, description) = if folder.title == self.section.title() {
                    self.section.empty()
                } else {
                    ("Folder Is Empty", "")
                };
                let empty = status_page()
                    .icon(icons::folder())
                    .title(title)
                    .description(description);

                // Invitations still show over an empty Shared with Me.
                return match self.invitations_group(tag, folder) {
                    Some(invitations) => adw::scrollable(
                        clamp(
                            column![invitations, empty.compact(true)]
                                .spacing(12)
                                .padding([24, 12]),
                        )
                        .maximum_size(860)
                        .tightening_threshold(600),
                    )
                    .boxed(),
                    None => empty.boxed(),
                };
            }
            Listing::Loaded(entries) => entries,
        };

        let _ = entries;
        let shown = folder.shown();

        let search = search_entry("Search this folder", &folder.filter)
            .on_input({
                let tag = tag.to_owned();
                move |filter| Message::Filtered(tag.clone(), filter)
            })
            .surface(Surface::View);

        let list: Element<'_, Message> = if shown.is_empty() {
            status_page()
                .icon(icons::system_search())
                .title("No Results Found")
                .description("Try a different search")
                .compact(true)
                .boxed()
        } else {
            // The view holds the selection and the keyboard: a click
            // selects, Ctrl and Shift add and extend, a drag rubberbands, a
            // double click or Enter opens, and a secondary press has the
            // menu — selecting its row alone first, as Files does.
            let on_select = {
                let tag = tag.to_owned();
                move |selection| Message::Selected(tag.clone(), selection)
            };
            let on_activate = {
                let tag = tag.to_owned();
                move |index| Message::Activated(tag.clone(), index)
            };
            let menu = {
                let (tag, shown, selection) = (tag.to_owned(), shown.clone(), &folder.selected);
                move |index: usize| self.item_menu(&tag, &shown, selection, index)
            };

            match self.config.view {
                View::List => {
                    let shown = shown.clone();
                    list_view(shown.len(), move |index| self.row(folder, shown[index]))
                        .selection(&folder.selected)
                        .on_select(on_select)
                        .on_activate(on_activate)
                        .context_menu(menu)
                        .rubberband(true)
                        .rich_list()
                        .id(view_id(tag))
                        .boxed()
                }
                View::Grid => {
                    let shown = shown.clone();
                    grid_view(shown.len(), move |index| self.tile(folder, shown[index]))
                        .selection(&folder.selected)
                        .on_select(on_select)
                        .on_activate(on_activate)
                        .context_menu(menu)
                        .rubberband(true)
                        .id(view_id(tag))
                        .boxed()
                }
            }
        };

        let mut page = iced::widget::Column::<Element<'_, Message>>::new()
            .spacing(12)
            .padding([24, 12]);
        if let Some(invitations) = self.invitations_group(tag, folder) {
            page = page.push(invitations);
        }
        let page = page.push(search.boxed()).push(list);

        adw::scrollable(clamp(page).maximum_size(860).tightening_threshold(600)).boxed()
    }

    /// The invitations waiting on the Shared with Me page, each with what
    /// was shared, by whom, and Accept and Decline — `None` without any.
    fn invitations_group<'a>(
        &'a self,
        tag: &str,
        folder: &'a Folder,
    ) -> Option<Element<'a, Message>> {
        if folder.invitations.is_empty() {
            return None;
        }

        let rows = folder.invitations.iter().map(|invitation| {
            let from = invitation.from.as_deref().map_or_else(
                || "Shared with you".to_owned(),
                |from| format!("From {from}"),
            );
            let accept = adw::text_button("Accept")
                .style(adw::button::suggested)
                .on_press(Message::AcceptInvitation(
                    tag.to_owned(),
                    invitation.clone(),
                ));
            let decline = adw::text_button("Decline").on_press(Message::RejectInvitation(
                tag.to_owned(),
                invitation.clone(),
            ));

            adw::action_row(invitation.name.as_str())
                .icon(match invitation.kind {
                    Kind::File => icons::text_x_generic(),
                    _ => icons::folder(),
                })
                .subtitle(format!("{from} · {}", invitation.role))
                .suffix(row![decline, accept].spacing(6))
        });

        Some(
            preferences_group()
                .title("Invitations")
                .extend(rows)
                .boxed(),
        )
    }

    /// The menu a secondary press on the item at `index` opens: it acts on
    /// the selection, which the view makes that item alone first unless
    /// the item is in it.
    fn item_menu(
        &self,
        tag: &str,
        shown: &[&Entry],
        selection: &Selection,
        index: usize,
    ) -> Vec<MenuEntry<Message>> {
        use libadwaita_iced::widget::popover_menu::item;

        let count = if selection.is_selected(index) {
            selection.iter().count()
        } else {
            1
        };
        let download = match count {
            1 => "Download 1 File…".to_owned(),
            count => format!("Download {count} Files…"),
        };

        let Some(entry) = shown.get(index) else {
            return Vec::new();
        };
        let mut entries: Vec<MenuEntry<Message>> = Vec::new();

        if self.section == Section::SharedWithMe && self.is_section_root(tag) {
            // What was shared with you is left, not trashed or renamed.
            entries.push(
                item(download)
                    .on_activate(Message::Download(tag.to_owned()))
                    .into(),
            );
            entries.push(
                item("Copy")
                    .accelerator("Ctrl+C")
                    .on_activate(Message::Copy(tag.to_owned()))
                    .into(),
            );
            entries.push(separator());
            entries.push(
                item("Leave")
                    .on_activate(Message::Leave(tag.to_owned(), entry.uid.clone()))
                    .into(),
            );
        } else if self.section == Section::Trash {
            // What the trash's items can have done, as Files offers it:
            // the CLI neither downloads nor renames them.
            let restore: Vec<Entry> = if selection.is_selected(index) {
                shown
                    .iter()
                    .enumerate()
                    .filter(|(at, _)| selection.is_selected(*at))
                    .map(|(_, entry)| (*entry).clone())
                    .collect()
            } else {
                vec![(*entry).clone()]
            };
            entries.push(
                item("Restore")
                    .on_activate(Message::Restore(tag.to_owned(), restore))
                    .into(),
            );
            entries.push(
                item("Delete Permanently…")
                    .accelerator("Delete")
                    .on_activate(Message::ConfirmDelete(tag.to_owned()))
                    .into(),
            );
        } else {
            entries.push(
                item(download)
                    .on_activate(Message::Download(tag.to_owned()))
                    .into(),
            );
            // One item is renamed, or shared; Files' batch rename is not
            // here.
            if count == 1 {
                entries.push(
                    item("Rename…")
                        .accelerator("F2")
                        .on_activate(Message::RenameIn(tag.to_owned(), entry.uid.clone()))
                        .into(),
                );
                entries.push(
                    item("Share…")
                        .on_activate(Message::ShareIn(tag.to_owned(), entry.uid.clone()))
                        .into(),
                );
            }
            entries.push(separator());
            entries.push(
                item("Cut")
                    .accelerator("Ctrl+X")
                    .on_activate(Message::Cut(tag.to_owned()))
                    .into(),
            );
            entries.push(
                item("Copy")
                    .accelerator("Ctrl+C")
                    .on_activate(Message::Copy(tag.to_owned()))
                    .into(),
            );
            if matches!(entry.kind, Kind::Folder) {
                // Files' "Paste Into Folder", sensitive with something
                // to paste.
                let paste = self
                    .clipboard
                    .as_ref()
                    .map(|_| Message::PasteInto(tag.to_owned(), entry.uid.clone()));
                entries.push(item("Paste Into Folder").on_activate_maybe(paste).into());
            }
            entries.push(separator());
            entries.push(
                item("Move to Trash")
                    .accelerator("Delete")
                    .on_activate(Message::Trash(tag.to_owned()))
                    .into(),
            );
        }

        entries.push(separator());
        entries.push(
            item("Info")
                .on_activate(Message::ShowInfo(tag.to_owned(), entry.uid.clone()))
                .into(),
        );
        entries
    }

    /// An entry in the grid: its icon large over its name, as a Files icon
    /// view lays it out.
    fn tile<'a>(&self, folder: &Folder, entry: &'a Entry) -> Element<'a, Message> {
        let glyph: Element<'a, Message> = if folder.opening.as_deref() == Some(entry.uid.as_str()) {
            spinner().size(32).boxed()
        } else {
            icon(entry_icon(entry)).size(48).boxed()
        };

        let name = typography::caption(entry.name.as_str())
            .width(Fill)
            .center()
            .wrapping(text::Wrapping::WordOrGlyph)
            .ellipsis(text::Ellipsis::End);

        column![
            center(glyph).height(64),
            // Two lines of the name at most.
            container(name).height(36).clip(true),
        ]
        .spacing(6)
        .align_x(Alignment::Center)
        .padding(6)
        .width(TILE_WIDTH)
        .boxed()
    }

    /// An entry in the list, laid out as an action row's header: the icon,
    /// the name over its details, and what the row ends in.
    fn row<'a>(&self, folder: &Folder, entry: &'a Entry) -> Element<'a, Message> {
        let mut details = Vec::new();
        if entry.kind == Kind::File
            && let Some(size) = entry.size
        {
            details.push(format::size(size));
        }
        if let Some(date) = entry.modified.as_deref().and_then(format::date) {
            details.push(date);
        }
        if entry.shared {
            details.push("Shared".to_owned());
        }

        let mut titles = column![typography::label(entry.name.as_str())]
            .spacing(row_metrics::TITLE_SPACING)
            .width(Fill);
        if !details.is_empty() {
            titles = titles.push(
                typography::subtitle(details.join(" · "))
                    .style(adw::text::dimmed)
                    .boxed(),
            );
        }

        let suffix: Option<Element<'a, Message>> =
            if folder.opening.as_deref() == Some(entry.uid.as_str()) {
                Some(spinner().boxed())
            } else if entry.is_folder() {
                Some(icon(icons::go_next()).boxed())
            } else {
                None
            };

        let mut header = row![
            container(icon(entry_icon(entry)))
                .padding(iced::padding::right(row_metrics::PREFIX_MARGIN)),
            titles,
        ]
        .spacing(row_metrics::SPACING)
        .align_y(Alignment::Center)
        .width(Fill);
        if let Some(suffix) = suffix {
            header = header.push(suffix);
        }

        header.boxed()
    }

    fn viewer<'a>(&'a self, tag: &'a str, viewer: &'a Viewer) -> Element<'a, Message> {
        match &viewer.body {
            Body::Text(content) => container(
                adw::text_editor(content)
                    .on_action({
                        let tag = tag.to_owned();
                        move |action| Message::Edited(tag.clone(), action)
                    })
                    .font(Font::MONOSPACE)
                    .padding(12)
                    .height(Fill)
                    .style(adw::text_editor::default),
            )
            .style(adw::container::view)
            .height(Fill)
            .boxed(),
            Body::Image(handle) => container(
                image::viewer(handle.clone())
                    .width(Fill)
                    .height(Fill)
                    .min_scale(0.05)
                    .max_scale(20.0),
            )
            .style(adw::container::view)
            .boxed(),
            Body::Svg(handle) => container(
                svg(handle.clone())
                    .style(adw::svg::full_color)
                    .width(Fill)
                    .height(Fill),
            )
            .padding(24)
            .style(adw::container::view)
            .boxed(),
        }
    }

    fn missing_cli(&self) -> Element<'_, Message> {
        let mut description = "Cold Drive browses Proton Drive through its command-line client. \
                               Install it and put proton-drive on your PATH, or set its path."
            .to_owned();
        if std::env::var_os(drive::CLI_ENV).is_some_and(|value| !value.is_empty()) {
            description.push_str(&format!(" {} points at no file.", drive::CLI_ENV));
        }

        self.lone_page(
            status_page()
                .icon(icons::dialog_error())
                .title("Proton Drive CLI Not Found")
                .description(description)
                .child(
                    column![
                        adw::pill_button("Get the CLI")
                            .style(adw::button::suggested_pill)
                            .on_press(Message::OpenExternally(CLI_DOWNLOAD.into())),
                        adw::pill_button("Set Its Path")
                            .on_press(Message::ShowDialog(Dialog::Preferences)),
                    ]
                    .spacing(12)
                    .align_x(Alignment::Center),
                ),
        )
    }

    fn signed_out_page(&self) -> Element<'_, Message> {
        let page = status_page()
            .icon(icons::network_workgroup())
            .title("Sign In to Proton Drive")
            .description(
                "Sign in once in your browser, on this device or another; the CLI keeps the \
                 session in your keyring.",
            );

        let Some(sign_in) = &self.sign_in else {
            return self.lone_page(
                page.child(
                    adw::pill_button("Sign In")
                        .style(adw::button::suggested_pill)
                        .on_press(Message::SignIn),
                ),
            );
        };

        let waiting = row![spinner(), typography::body("Waiting for the browser…")]
            .spacing(12)
            .align_y(Alignment::Center);

        let mut actions = column![waiting].spacing(18).align_x(Alignment::Center);
        if let Some(url) = &sign_in.url {
            actions = actions.push(
                row![
                    adw::icon_text_button(icons::adw_external_link(), "Open Browser")
                        .on_press(Message::OpenExternally(url.clone())),
                    adw::button(
                        row![
                            self.copy_icon("Sign-in link"),
                            typography::heading("Copy Link")
                        ]
                        .spacing(6)
                        .align_y(Alignment::Center),
                    )
                    .icon_text_button()
                    .on_press(Message::CopyText(url.clone(), "Sign-in link")),
                ]
                .spacing(12)
                .boxed(),
            );
        }
        actions = actions.push(
            adw::pill_button("Cancel")
                .on_press(Message::CancelSignIn)
                .boxed(),
        );

        self.lone_page(page.child(actions))
    }

    /// A page with nothing beside it: the status pages before the drive.
    fn lone_page<'a>(&self, content: impl Widget<Message> + 'a) -> Element<'a, Message> {
        let backdrop = !self.focused;

        toolbar_view(content)
            .top(
                header_bar()
                    .title(window_title("Cold Drive").backdrop(backdrop))
                    .on_window(Message::Window)
                    .maximized(self.maximized)
                    .backdrop(backdrop),
            )
            .backdrop(backdrop)
            .boxed()
    }
}

/// Fetches `entry` — from the cache if this revision was opened before —
/// and reads it as what it looks to be.
async fn open(cli: Cli, entry: Entry) -> Result<Opened, drive::Error> {
    let dir = files::cache_dir(&entry);

    let file = match files::cached(&dir).await {
        Some(file) => file,
        None => {
            // Downloaded beside, and moved in once whole, so an interrupted
            // download is never taken for the file.
            let partial = dir.with_extension("part");
            let _ = tokio::fs::remove_dir_all(&partial).await;
            let file = cli.download(entry.path.clone(), partial.clone()).await?;
            let name = file.file_name().map(ToOwned::to_owned).unwrap_or_default();
            let _ = tokio::fs::remove_dir_all(&dir).await;
            tokio::fs::rename(&partial, &dir).await?;
            dir.join(name)
        }
    };

    files::load(file, files::classify(&entry)).await
}

/// The menu to add to a folder: files from the computer, or a new folder.
/// Whether the window draws its own rounded corners over a transparent
/// surface: Linux compositors leave the corners clear, where macOS and
/// Windows show black, so there the window is square and opaque.
pub const ROUNDED_WINDOW: bool = cfg!(target_os = "linux");

/// The message a key no widget took stands for, if any.
fn shortcut(event: keyboard::Event) -> Option<Message> {
    match event {
        // `?` is Shift and another key on most layouts, so it is matched
        // as typed, as GTK's `<Control>question` is.
        keyboard::Event::KeyPressed {
            modified_key: Key::Character(character),
            modifiers,
            ..
        } if character == "?" && modifiers.command() => {
            Some(Message::ShowDialog(Dialog::Shortcuts))
        }
        keyboard::Event::KeyPressed { key, modifiers, .. } => match key.as_ref() {
            Key::Named(Named::Escape) => Some(Message::Escape),
            Key::Named(Named::F5) => Some(Message::RefreshTop),
            Key::Character("r") if modifiers.command() => Some(Message::RefreshTop),
            Key::Character("u") if modifiers.command() => Some(Message::UploadHere),
            Key::Character("n") if modifiers.command() => Some(Message::NewFolderHere),
            Key::Character("c") if modifiers.command() => Some(Message::CopySelected),
            Key::Character("x") if modifiers.command() => Some(Message::CutSelected),
            Key::Character("v") if modifiers.command() => Some(Message::PasteHere),
            Key::Named(Named::F2) => Some(Message::RenameSelected),
            Key::Named(Named::Delete) => Some(Message::DeleteSelected),
            Key::Character(",") if modifiers.command() => {
                Some(Message::ShowDialog(Dialog::Preferences))
            }
            Key::Named(Named::ArrowLeft | Named::ArrowUp) if modifiers.alt() => Some(Message::Back),
            _ => None,
        },
        _ => None,
    }
}

fn add_menu(tag: &str) -> Element<'static, Message> {
    menu_button(icon(icons::list_add()))
        .style(adw::button::flat)
        .image_button()
        .push(
            item("Upload Files…")
                .accelerator("Ctrl+U")
                .on_activate(Message::UploadTo(tag.to_owned())),
        )
        .push(
            item("New Folder…")
                .accelerator("Ctrl+N")
                .on_activate(Message::NewFolderIn(tag.to_owned())),
        )
        .boxed()
}

/// What a transfer's toast says: what failed, if anything did — `verb` the
/// transfer, "upload" or "download".
fn transfer_failure(verb: &str, transfer: &Transfer) -> Option<String> {
    match transfer.failures.as_slice() {
        [] => None,
        [(name, Some(reason))] => Some(format!("Could not {verb} “{name}”: {reason}")),
        [(name, None)] => Some(format!("Could not {verb} “{name}”")),
        failures => Some(format!("Could not {verb} {} items", failures.len())),
    }
}

/// A page on the view surface throughout — `window.view`, as Files has
/// it — with its flat bars showing that surface through them.
fn on_view<'a>(page: impl Widget<Message> + 'a) -> Element<'a, Message> {
    container(page)
        .style(adw::container::view)
        .width(Fill)
        .height(Fill)
        .boxed()
}

/// Whether `text` could be an address to invite: something, an `@`, and
/// something with a dot after it. The server decides the rest.
fn looks_like_email(text: &str) -> bool {
    matches!(text.split_once('@'), Some((user, host)) if !user.is_empty() && host.contains('.') && !host.ends_with('.'))
}

/// `entries` as a toast or an operation names them: the one by name, more
/// by their count.
fn describe(entries: &[Entry]) -> String {
    match entries {
        [entry] => format!("“{}”", entry.name),
        entries => format!("{} items", entries.len()),
    }
}

/// The toast for a node operation that failed some of its items.
fn failed(verb: &str, where_to: &str, done: &Done) -> String {
    let to = if where_to.is_empty() {
        String::new()
    } else {
        format!(" {where_to}")
    };
    match done.failed.as_slice() {
        [(name, Some(reason))] => format!("Could not {verb} “{name}”{to}: {reason}"),
        [(name, None)] => format!("Could not {verb} “{name}”{to}"),
        failed => format!("Could not {verb} {} items{to}", failed.len()),
    }
}

/// A banner saying what was taken — "“Notes.md” copied", "3 items cut" —
/// with Paste Here and a close, laid out and tinted as `AdwBanner` is;
/// `AdwBanner` has no close, so this is composed. Slid away with `None`.
fn taken_banner<'a>(
    clipboard: Option<&Clipboard>,
    on_paste: Message,
    backdrop: bool,
) -> Element<'a, Message> {
    use libadwaita_iced::widget::banner;
    use libadwaita_iced::widget::revealer::{Transition, revealer};

    let title = clipboard.map_or_else(String::new, |clipboard| {
        let what = describe(&clipboard.entries);
        if clipboard.cut {
            format!("{what} cut")
        } else {
            format!("{what} copied")
        }
    });
    let bar = row![
        container(typography::heading(title))
            .width(Fill)
            .padding(iced::padding::left(6.0)),
        adw::text_button("Paste Here").on_press(on_paste),
        adw::circular_button(icons::window_close())
            .style(adw::button::flat)
            .on_press(Message::DismissTaken),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    revealer(
        container(bar)
            .padding(6)
            .width(Fill)
            .style(move |theme: &Adwaita| iced::widget::container::Style {
                background: Some(banner::background(theme, Surface::View, backdrop).into()),
                ..Default::default()
            }),
    )
    .revealed(clipboard.is_some())
    .transition(Transition::SlideDown)
    .boxed()
}

/// The id of a folder page's list or grid, to scroll it by.
fn view_id(tag: &str) -> iced::advanced::widget::Id {
    format!("{tag}/view").into()
}

/// A page's title, over a subtitle only when there is one: an empty
/// subtitle still takes its line, and lifts the title off centre.
fn page_title<'a>(
    title: &'a str,
    subtitle: Option<&str>,
    backdrop: bool,
) -> adw::window_title::WindowTitle<'a, Message> {
    let title = window_title(title).backdrop(backdrop);

    match subtitle.filter(|subtitle| !subtitle.is_empty()) {
        Some(subtitle) => title.subtitle(subtitle.to_owned()),
        None => title,
    }
}

fn entry_icon(entry: &Entry) -> svg::Handle {
    match entry.kind {
        Kind::Device => return icons::computer(),
        Kind::Folder => return icons::folder(),
        Kind::File => {}
    }

    let media = entry.media_type.as_deref().unwrap_or_default();
    match files::classify(entry) {
        Class::Image | Class::Svg => icons::image_x_generic(),
        Class::Document(Document::Doc) => icons::x_office_document(),
        Class::Document(Document::Sheet) => icons::x_office_spreadsheet(),
        Class::Text => icons::text_x_generic(),
        Class::Other if media.starts_with("audio/") => icons::audio_x_generic(),
        Class::Other if media.starts_with("video/") => icons::video_x_generic(),
        Class::Other if media.contains("spreadsheet") || media.contains("excel") => {
            icons::x_office_spreadsheet()
        }
        Class::Other if media.contains("presentation") || media.contains("powerpoint") => {
            icons::x_office_presentation()
        }
        Class::Other
            if media == "application/pdf"
                || media.contains("document")
                || media.contains("msword")
                || media.contains("rtf") =>
        {
            icons::x_office_document()
        }
        Class::Other
            if [
                "zip",
                "tar",
                "gzip",
                "bzip",
                "xz",
                "7z",
                "rar",
                "compressed",
                "zstd",
            ]
            .iter()
            .any(|kind| media.contains(kind)) =>
        {
            icons::package_x_generic()
        }
        Class::Other if media.contains("executable") || media.contains("x-msdownload") => {
            icons::application_x_executable()
        }
        Class::Other => icons::text_x_generic(),
    }
}

impl Section {
    const ALL: [Section; 5] = [
        Section::MyFiles,
        Section::Devices,
        Section::SharedWithMe,
        Section::SharedByMe,
        Section::Trash,
    ];

    fn title(self) -> &'static str {
        match self {
            Section::MyFiles => "My Files",
            Section::Devices => "Computers",
            Section::SharedWithMe => "Shared with Me",
            Section::SharedByMe => "Shared by Me",
            Section::Trash => "Trash",
        }
    }

    fn path(self) -> &'static str {
        match self {
            Section::MyFiles => "/my-files",
            Section::Devices => "/devices",
            Section::SharedWithMe => "/shared-with-me",
            Section::SharedByMe => "/shared-by-me",
            Section::Trash => "/trash",
        }
    }

    fn icon(self) -> svg::Handle {
        match self {
            Section::MyFiles => icons::user_home(),
            Section::Devices => icons::computer(),
            Section::SharedWithMe => icons::folder_remote(),
            Section::SharedByMe => icons::folder_publicshare(),
            Section::Trash => icons::user_trash(),
        }
    }

    /// The empty state's title and description.
    fn empty(self) -> (&'static str, &'static str) {
        match self {
            Section::MyFiles => ("No Files", "Files uploaded to Proton Drive show here"),
            Section::Devices => (
                "No Computers",
                "Computers synced with Proton Drive show here",
            ),
            Section::SharedWithMe => (
                "Nothing Shared with You",
                "Files others share with you show here",
            ),
            Section::SharedByMe => ("Nothing Shared", "Files you share show here"),
            Section::Trash => ("Trash Is Empty", ""),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    use iced_test::Simulator;

    fn entry(name: &str, kind: Kind) -> Entry {
        Entry {
            uid: name.to_owned(),
            name: name.to_owned(),
            kind,
            media_type: None,
            size: None,
            modified: None,
            shared: false,
            revision: None,
            path: format!("/my-files/{name}"),
        }
    }

    /// An app showing My Files listed as `entries`, with no CLI behind it.
    fn showing(entries: Vec<Entry>) -> App {
        let config = Config {
            cli_path: None,
            view: View::List,
        };
        let mut app = App::blank(config, Some(Cli::never_run()));
        app.push(PageKind::Folder(Folder {
            title: Section::MyFiles.title().into(),
            trail: Section::MyFiles.title().into(),
            path: Section::MyFiles.path().into(),
            listing: Listing::Loaded(entries),
            filter: String::new(),
            opening: None,
            writable: true,
            transfers: 0,
            select_next: Vec::new(),
            invitations: Vec::new(),
            selected: Selection::Multiple(Default::default()),
        }));
        app
    }

    /// A click on the first row of the one folder page's list — the
    /// row's name is a typography text, which no text selector sees.
    fn click_first_row(ui: &mut Simulator<'_, Message, Adwaita>) {
        // The list hands no id to a container operation, so it is found
        // by what it is: the one scrollable's content, below the search
        // entry — the text input — with the first row at its top.
        let mut found = None;
        let _ = ui.find(|candidate: iced_test::selector::Candidate<'_>| {
            if let iced_test::selector::Candidate::TextInput { bounds, .. } = candidate {
                found = Some(bounds);
            }
            None::<()>
        });
        let search = found.expect("the search entry");
        // 12px below the entry, then 20px into the first row.
        ui.point_at(iced::Point::new(
            search.center_x(),
            search.y + search.height + 12.0 + 20.0,
        ));
        let _ = ui.simulate(iced_test::simulator::click());
    }

    /// The first click on a row selects it, and the app shows the
    /// selection; the second, on the view rebuilt, opens it — with the
    /// view's state carried over, as iced carries it.
    #[test]
    fn a_second_click_opens_the_folder_the_first_selected() {
        assert!(second_click_opens(Duration::ZERO));
    }

    /// The same with the pointer resting on the row first: the list once
    /// stamped a press with the time of the last frame, not its own, and
    /// this needed a third click.
    #[test]
    fn a_second_click_opens_the_folder_after_the_pointer_rested() {
        assert!(second_click_opens(
            list_view::DOUBLE_CLICK + Duration::from_millis(100)
        ));
    }

    fn second_click_opens(rest: Duration) -> bool {
        let entries = || vec![entry("Docs", Kind::Folder), entry("Notes.md", Kind::File)];
        let before = showing(entries());
        let mut after = showing(entries());
        let theme = before.theme();

        let mut ui = Simulator::with_size(typography::settings(), (960.0, 640.0), before.view());
        ui.draw(&theme);
        std::thread::sleep(rest);
        click_first_row(&mut ui);
        let messages: Vec<Message> = ui.drain().collect();
        assert!(
            messages
                .iter()
                .any(|message| matches!(message, Message::Selected(..))),
            "the first click selects: {messages:?}"
        );
        for message in messages {
            let _ = after.update(message);
        }

        let mut ui = ui.rebuild(after.view());
        ui.draw(&theme);
        click_first_row(&mut ui);
        ui.drain()
            .any(|message| matches!(message, Message::Activated(_, 0)))
    }

    fn upload(uploaded: u64, skipped: u64, failures: &[(&str, Option<&str>)]) -> Transfer {
        Transfer {
            transferred: uploaded,
            skipped,
            failures: failures
                .iter()
                .map(|(name, reason)| ((*name).to_owned(), reason.map(str::to_owned)))
                .collect(),
        }
    }

    #[test]
    fn addresses_are_checked_loosely() {
        assert!(looks_like_email("ann@proton.me"));
        assert!(!looks_like_email("ann"));
        assert!(!looks_like_email("@proton.me"));
        assert!(!looks_like_email("ann@proton."));
    }

    #[test]
    fn only_failures_are_told() {
        assert_eq!(transfer_failure("upload", &upload(3, 1, &[])), None);
        assert_eq!(
            transfer_failure("upload", &upload(0, 0, &[("a.txt", Some("Too big"))])),
            Some("Could not upload “a.txt”: Too big".to_owned())
        );
        assert_eq!(
            transfer_failure("upload", &upload(1, 0, &[("b", None)])),
            Some("Could not upload “b”".to_owned())
        );
        assert_eq!(
            transfer_failure("upload", &upload(1, 0, &[("b", None), ("c", None)])),
            Some("Could not upload 2 items".to_owned())
        );
    }
}
