//! The window: a sidebar of Drive's sections beside a navigation stack of
//! folders, with files opened onto it as pages of their own.

use std::path::PathBuf;

use iced::keyboard::{self, Key, key::Named};
use iced::theme::Mode;
use iced::widget::grid::Sizing;
use iced::widget::{center, column, container, grid, image, row, svg, text, text_editor};
use iced::{Alignment, Fill, Font, Subscription, Task};
use libadwaita_iced::widget::about_dialog::Page as AboutPage;
use libadwaita_iced::widget::breakpoint_bin::{self, breakpoint_bin};
use libadwaita_iced::widget::navigation_view::NavigationPage;
use libadwaita_iced::widget::popover_menu::{item, menu_button, separator};
use libadwaita_iced::widget::sidebar::{self, Mode as SidebarMode};
use libadwaita_iced::widget::toast::{self, Toasts};
use libadwaita_iced::widget::{
    action_row, boxed_list, clamp, dialog, header_bar, icon, navigation_page,
    navigation_split_view, navigation_view, search_entry, spinner, status_page, toast_overlay,
    toolbar_view, window_title,
};
use libadwaita_iced::{
    AccentColor, Adwaita, ColorScheme, Contrast, Element, Widget, icons, typography, widget as adw,
    window,
};

use crate::config::{Config, View};
use crate::drive::{self, Account, Cli, Entry, Kind, Login};
use crate::files::{self, Class, Content, Opened};
use crate::{format, icons as more_icons};

mod dialogs;

/// A grid cell's widest; the columns are as many as fit.
const TILE_WIDTH: f32 = 128.0;

/// Width over height of a grid cell: the icon over two lines of name.
const TILE_ASPECT_RATIO: f32 = 0.95;

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialog {
    Preferences,
    About,
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
    Activated(String, String),
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
                keyboard::Event::KeyPressed { key, modifiers, .. } => match key.as_ref() {
                    Key::Named(Named::F5) => Some(Message::RefreshTop),
                    Key::Character("r") if modifiers.command() => Some(Message::RefreshTop),
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
                        Ok(entries) => Listing::Loaded(entries),
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
            Message::Activated(tag, uid) => return self.activate(&tag, &uid),
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
            Message::DialogClosed => self.dialog = None,
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
            };
            let child = self.push(PageKind::Folder(child));
            return self.load(&child);
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
            .on_pop(Message::Back);

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
                    View::List => more_icons::view_grid(),
                    View::Grid => more_icons::view_list(),
                };
                let bar = self
                    .content_bar(
                        page_title(&folder.title, parent, backdrop),
                        depth,
                        collapsed,
                    )
                    .end(row![
                        adw::icon_button(toggle)
                            .style(adw::button::flat)
                            .on_press(Message::ToggleView),
                        adw::icon_button(more_icons::view_refresh())
                            .style(adw::button::flat)
                            .on_press(Message::Reload(page.tag.clone())),
                    ]);

                (
                    folder.title.as_str(),
                    toolbar_view(self.folder(&page.tag, folder))
                        .top(bar)
                        .backdrop(backdrop)
                        .boxed(),
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
                        adw::icon_button(icons::external_link())
                            .style(adw::button::flat)
                            .on_press(Message::OpenExternally(
                                viewer.file.to_string_lossy().into_owned(),
                            )),
                    );

                (
                    viewer.entry.name.as_str(),
                    toolbar_view(self.viewer(&page.tag, viewer))
                        .top(bar)
                        .backdrop(backdrop)
                        .boxed(),
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
                    .icon(more_icons::network_offline())
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
                    .icon(more_icons::folder())
                    .title(title)
                    .description(description)
                    .boxed();
            }
            Listing::Loaded(entries) => entries,
        };

        let needle = folder.filter.to_lowercase();
        let shown: Vec<&Entry> = entries
            .iter()
            .filter(|entry| needle.is_empty() || entry.name.to_lowercase().contains(&needle))
            .collect();

        let search = search_entry("Search this folder", &folder.filter).on_input({
            let tag = tag.to_owned();
            move |filter| Message::Filtered(tag.clone(), filter)
        });

        let list: Element<'_, Message> = if shown.is_empty() {
            status_page()
                .icon(icons::system_search())
                .title("No Results Found")
                .description("Try a different search")
                .compact(true)
                .boxed()
        } else {
            match self.config.view {
                View::List => boxed_list()
                    .extend(shown.into_iter().map(|entry| self.row(tag, folder, entry)))
                    .boxed(),
                View::Grid => grid(shown.into_iter().map(|entry| self.tile(tag, folder, entry)))
                    .fluid(TILE_WIDTH)
                    .spacing(6)
                    .height(Sizing::AspectRatio(TILE_ASPECT_RATIO))
                    .boxed(),
            }
        };

        adw::scrollable(
            clamp(column![search, list].spacing(12).padding([24, 12]))
                .maximum_size(860)
                .tightening_threshold(600),
        )
        .boxed()
    }

    /// An entry in the grid: its icon large over its name, the whole a
    /// flat button, as a Files icon view lays it out.
    fn tile<'a>(&self, tag: &str, folder: &Folder, entry: &'a Entry) -> Element<'a, Message> {
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

        adw::button(
            column![
                center(glyph).height(64),
                // Two lines of the name at most.
                container(name).height(36).clip(true),
            ]
            .spacing(6)
            .align_x(Alignment::Center),
        )
        .style(adw::button::flat)
        .width(Fill)
        .height(Fill)
        .padding(6)
        .on_press(Message::Activated(tag.to_owned(), entry.uid.clone()))
        .boxed()
    }

    fn row<'a>(
        &self,
        tag: &str,
        folder: &Folder,
        entry: &'a Entry,
    ) -> adw::action_row::ActionRow<'a, Message> {
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

        let mut row = action_row(entry.name.as_str())
            .icon(entry_icon(entry))
            .on_activate(Message::Activated(tag.to_owned(), entry.uid.clone()));
        if !details.is_empty() {
            row = row.subtitle(details.join(" · "));
        }

        if folder.opening.as_deref() == Some(entry.uid.as_str()) {
            row.suffix(spinner())
        } else if entry.is_folder() {
            row.suffix(icon(icons::go_next()))
        } else {
            row
        }
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
            .icon(more_icons::network_workgroup())
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
                    adw::icon_text_button(icons::external_link(), "Open Browser")
                        .on_press(Message::OpenExternally(url.clone())),
                    adw::icon_text_button(more_icons::edit_copy(), "Copy Link")
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
        Kind::Device => return more_icons::computer(),
        Kind::Folder => return more_icons::folder(),
        Kind::File => {}
    }

    let media = entry.media_type.as_deref().unwrap_or_default();
    match files::classify(entry) {
        Class::Image | Class::Svg => more_icons::image_x_generic(),
        Class::Text => more_icons::text_x_generic(),
        Class::Other if media.starts_with("audio/") => more_icons::audio_x_generic(),
        Class::Other if media.starts_with("video/") => more_icons::video_x_generic(),
        Class::Other if media.contains("spreadsheet") || media.contains("excel") => {
            more_icons::x_office_spreadsheet()
        }
        Class::Other if media.contains("presentation") || media.contains("powerpoint") => {
            more_icons::x_office_presentation()
        }
        Class::Other
            if media == "application/pdf"
                || media.contains("document")
                || media.contains("msword")
                || media.contains("rtf") =>
        {
            more_icons::x_office_document()
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
            more_icons::package_x_generic()
        }
        Class::Other if media.contains("executable") || media.contains("x-msdownload") => {
            more_icons::application_x_executable()
        }
        Class::Other => more_icons::text_x_generic(),
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
            Section::MyFiles => more_icons::user_home(),
            Section::Devices => more_icons::computer(),
            Section::SharedWithMe => more_icons::folder_remote(),
            Section::SharedByMe => more_icons::folder_publicshare(),
            Section::Trash => more_icons::user_trash(),
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
