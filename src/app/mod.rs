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
use libadwaita_iced::widget::{action_row as row_metrics, grid_view, list_view};
use libadwaita_iced::widget::{
    clamp, dialog, header_bar, icon, navigation_page, navigation_split_view, navigation_view,
    search_entry, spinner, status_page, toast_overlay, toolbar_view, window_title,
};
use libadwaita_iced::{
    AccentColor, Adwaita, ColorScheme, Contrast, Element, Widget, icons, metrics, typography,
    widget as adw, window,
};

use crate::config::{Config, View};
use crate::drive::{self, Account, Cli, Details, Entry, Kind, Login, Transfer};
use crate::files::{self, Class, Content, Document, Opened};
use crate::format;

mod dialogs;

/// A grid cell's widest; the columns are as many as fit.
const TILE_WIDTH: f32 = 128.0;

/// Where the CLI is to be had.
const CLI_DOWNLOAD: &str = "https://proton.me/download/drive/cli/index.html";

pub struct App {
    config: Config,
    cli: Option<Cli>,
    section: Section,
    /// Collapsed, whether the content is up rather than the sidebar.
    show_content: bool,
    /// The content's navigation stack, bottom first. Popped pages stay until
    /// they have slid out of view.
    pages: Vec<Page>,
    next_tag: u64,
    /// The CLI has no session.
    signed_out: bool,
    sign_in: Option<SignIn>,
    toasts: Toasts<Message>,
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
    info: Option<Info>,
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
    /// Escape, outside a dialog: whatever is up first goes.
    Escape,
    Download(String),
    DownloadTo(String, Vec<String>, Option<PathBuf>),
    Downloaded(String, Result<Transfer, drive::Error>),
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
    CopyText(String),
    Copied,
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
    Uploaded(String, Vec<String>, Result<Transfer, drive::Error>),
    NewFolderIn(String),
    NewFolderHere,
    /// Rename the item, by the page it is on and its UID.
    RenameIn(String, String),
    /// Rename the one item selected, F2.
    RenameSelected,
    NameTyped(String),
    NameSubmitted,
    FolderCreated(String, String, Result<(), drive::Error>),
    /// The page, the old name, the new name, and how it went.
    Renamed(String, String, String, Result<(), drive::Error>),
}

impl App {
    pub fn new() -> (Self, Task<Message>) {
        // Decrypted copies are kept for one session only.
        let _ = std::fs::remove_dir_all(files::cache_dir_root());

        let config = Config::load();
        let mut app = Self {
            cli: Cli::locate(config.cli_path.as_deref()),
            cli_path_input: config
                .cli_path
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            config,
            section: Section::MyFiles,
            show_content: false,
            pages: Vec::new(),
            next_tag: 0,
            signed_out: false,
            sign_in: None,
            toasts: Toasts::new(),
            scheme: ColorScheme::Light,
            focused: true,
            maximized: false,
            dialog: None,
            dialog_open: false,
            cli_version: None,
            account: None,
            about_pages: Vec::new(),
            naming: None,
            info: None,
        };

        let load = app.open_section(Section::MyFiles);
        (
            app,
            Task::batch([iced::system::theme().map(Message::SystemScheme), load]),
        )
    }

    pub fn theme(&self) -> Adwaita {
        Adwaita::new(self.scheme, Contrast::Normal, AccentColor::Purple)
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            window::focus_changes().map(Message::Focused),
            iced::window::resize_events().map(|_| Message::Resized),
            iced::system::theme_changes().map(Message::SystemScheme),
            keyboard::listen().filter_map(|event| match event {
                // `?` is Shift and another key on most layouts, so it is
                // matched as typed, as GTK's `<Control>question` is.
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
                    Key::Named(Named::F2) => Some(Message::RenameSelected),
                    Key::Character(",") if modifiers.command() => {
                        Some(Message::ShowDialog(Dialog::Preferences))
                    }
                    Key::Named(Named::ArrowLeft | Named::ArrowUp) if modifiers.alt() => {
                        Some(Message::Back)
                    }
                    _ => None,
                },
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
            Message::Downloaded(tag, result) => {
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
            Message::CopyText(text) => {
                return iced::clipboard::write(text).map(|_| Message::Copied);
            }
            Message::Copied => self.toast("Copied to clipboard".into()),
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
            Message::Uploaded(tag, names, result) => return self.uploaded(&tag, names, result),
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
            Message::Renamed(tag, old, new, result) => match result {
                Ok(()) => {
                    tracing::info!(from = %old, to = %new, "renamed");
                    if let Some(folder) = self.folder_mut(&tag) {
                        folder.select_next = vec![new];
                    }
                    return self.load(&tag);
                }
                Err(drive::Error::AuthRequired) => self.signed_out = true,
                Err(error) => self.toast(format!("Could not rename “{old}”: {error}")),
            },
            Message::FolderCreated(tag, name, result) => match result {
                Ok(()) => {
                    tracing::info!(%name, "folder created");
                    if let Some(folder) = self.folder_mut(&tag) {
                        folder.select_next = vec![name];
                    }
                    return self.load(&tag);
                }
                Err(drive::Error::AuthRequired) => self.signed_out = true,
                Err(error) => self.toast(format!("Could not create “{name}”: {error}")),
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
            .rounded(true)
            .boxed()
    }

    // ------------------------------------------------------------ navigation

    /// Replaces the stack with `section`'s root, and lists it.
    fn open_section(&mut self, section: Section) -> Task<Message> {
        self.section = section;
        self.pages.clear();

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
        let tag = tag.to_owned();

        Task::perform(cli.list(folder.path.clone()), move |result| {
            Message::Listed(tag.clone(), result)
        })
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

        let tag = tag.to_owned();
        Task::perform(cli.download_to(paths, dir), move |result| {
            Message::Downloaded(tag.clone(), result)
        })
    }

    /// The folder on top of the stack, if files and folders can go in it.
    fn writable_top(&self) -> Option<String> {
        self.live_pages()
            .last()
            .filter(|page| matches!(&page.kind, PageKind::Folder(folder) if folder.writable))
            .map(|page| page.tag.clone())
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

        // The spinner in the header bar shows it is under way.
        let tag = tag.to_owned();
        Task::perform(cli.upload(files, parent), move |result| {
            Message::Uploaded(tag.clone(), names.clone(), result)
        })
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
        let Some(parent) = self.folder_mut(&tag).map(|folder| folder.path.clone()) else {
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
                Task::perform(cli.rename(entry.path, name.clone()), move |result| {
                    Message::Renamed(tag.clone(), old.clone(), name.clone(), result)
                })
            }
            None => {
                tracing::info!(%name, %parent, "creating a folder");
                Task::perform(cli.create_folder(parent, name.clone()), move |result| {
                    Message::FolderCreated(tag.clone(), name.clone(), result)
                })
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

        let stack = navigation_view()
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
            .push(item("About Cold Pass").on_activate(Message::ShowDialog(Dialog::About)))
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

                (
                    folder.title.as_str(),
                    // The whole pane is the view surface, bar included, as
                    // Files' window carries `.view` under a flat bar: the
                    // list paints `--view-bg-color` under its rows, and
                    // nothing comes between the bar and them.
                    on_view(
                        toolbar_view(self.folder(&page.tag, folder))
                            .top(bar)
                            .backdrop(backdrop),
                    ),
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
                let bar = self
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

        navigation_page(page.tag.clone(), view)
            .title(title)
            .on_hidden(Message::PageHidden(page.tag.clone()))
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
                return status_page()
                    .icon(icons::folder())
                    .title(title)
                    .description(description)
                    .boxed();
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

        adw::scrollable(
            clamp(column![search, list].spacing(12).padding([24, 12]))
                .maximum_size(860)
                .tightening_threshold(600),
        )
        .boxed()
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

        let mut entries = vec![
            item(download)
                .on_activate(Message::Download(tag.to_owned()))
                .into(),
        ];
        if let Some(entry) = shown.get(index) {
            // One item is renamed; Files' batch rename is not here.
            if count == 1 {
                entries.push(
                    item("Rename…")
                        .accelerator("F2")
                        .on_activate(Message::RenameIn(tag.to_owned(), entry.uid.clone()))
                        .into(),
                );
            }
            entries.push(
                item("Info")
                    .on_activate(Message::ShowInfo(tag.to_owned(), entry.uid.clone()))
                    .into(),
            );
        }
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
        let mut description = "Cold Pass browses Proton Drive through its command-line client. \
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
                    adw::icon_text_button(icons::edit_copy(), "Copy Link")
                        .on_press(Message::CopyText(url.clone())),
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
                    .title(window_title("Cold Pass").backdrop(backdrop))
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
