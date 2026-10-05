//! The `proton-drive` command-line client, driven as a child process.
//!
//! Every call runs one command with `--json` and parses what it prints. Nodes
//! are addressed as `/my-files/<uid>`: the CLI resolves a UID segment with a
//! direct lookup wherever it sits in a path, so this reaches any node the
//! account can see — in My Files, on a device, shared with the account or in
//! the trash — without walking the tree by name.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Instant;

use iced::futures::{SinkExt, Stream};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;

/// Overrides where the CLI is looked for.
pub const CLI_ENV: &str = "COLD_PASS_DRIVE_CLI";

/// The executable names the CLI ships under.
const CLI_NAMES: &[&str] = &["proton-drive", "proton-drive-cli"];

/// What the CLI prints, on stderr, for a command that needs a session.
const AUTH_REQUIRED: &str = "You need to login first";

#[derive(Debug, Clone)]
pub struct Cli {
    program: PathBuf,
    source: Source,
}

/// Where the CLI in use was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// [`CLI_ENV`].
    Environment,
    Settings,
    Path,
}

/// The signed-in account, as far as the CLI tells.
#[derive(Debug, Clone)]
pub struct Account {
    pub email: Option<String>,
    /// What My Files holds; `None` from a CLI without `filesystem size`.
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Copy)]
pub struct Usage {
    /// Bytes, trashed ones included.
    pub bytes: u64,
    pub items: u64,
}

#[derive(Debug, Clone)]
pub enum Error {
    /// There is no session; `auth login` first.
    AuthRequired,
    /// The CLI ran and refused, with what it said.
    Failed(String),
    /// The CLI could not be run, or its output read.
    Io(String),
    /// The CLI printed something that is not the JSON expected.
    Parse(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::AuthRequired => f.write_str("Signed out of Proton Drive"),
            Error::Failed(message) | Error::Io(message) => f.write_str(message),
            Error::Parse(message) => write!(f, "Unexpected output from the CLI: {message}"),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Error::Io(error.to_string())
    }
}

/// A file or folder in a listing.
#[derive(Debug, Clone)]
pub struct Entry {
    pub uid: String,
    pub name: String,
    pub kind: Kind,
    pub media_type: Option<String>,
    pub size: Option<u64>,
    /// When the content last changed: the file's own modification time if
    /// it was uploaded with one, the server's otherwise. RFC 3339.
    pub modified: Option<String>,
    pub shared: bool,
    /// The active revision, which keys the download cache.
    pub revision: Option<String>,
    /// The path to hand the CLI for this node.
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Folder,
    File,
    Device,
}

impl Entry {
    /// Where the entry is addressed once in the trash.
    pub fn trash_path(&self) -> String {
        trash_path(&self.name)
    }

    pub fn is_folder(&self) -> bool {
        matches!(self.kind, Kind::Folder | Kind::Device)
    }
}

/// The steps of `auth login`.
#[derive(Debug, Clone)]
pub enum Login {
    /// The page to sign in at, which the CLI has also tried to open.
    Url(String),
    Done(Result<(), Error>),
}

impl Cli {
    /// Finds the CLI: [`CLI_ENV`] if set, else `configured` if it is a
    /// file, else the first of [`CLI_NAMES`] on `PATH` or in `~/.local/bin`.
    pub fn locate(configured: Option<&Path>) -> Option<Self> {
        if let Some(program) = std::env::var_os(CLI_ENV).filter(|value| !value.is_empty()) {
            return Some(Self::found(program.into(), Source::Environment));
        }

        if let Some(program) = configured {
            if program.is_file() {
                return Some(Self::found(program.to_owned(), Source::Settings));
            }
            tracing::warn!(path = %program.display(), "the CLI set in the settings is not a file");
        }

        let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).collect())
            .unwrap_or_default();
        dirs.extend(dirs::executable_dir());

        let found = dirs
            .iter()
            .flat_map(|dir| CLI_NAMES.iter().map(move |name| dir.join(executable(name))))
            .find(|candidate| candidate.is_file())
            .map(|program| Self::found(program, Source::Path));

        if found.is_none() {
            tracing::warn!("no Proton Drive CLI on PATH");
        }
        found
    }

    fn found(program: PathBuf, source: Source) -> Self {
        tracing::info!(program = %program.display(), ?source, "using the Proton Drive CLI");
        Self { program, source }
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    pub fn source(&self) -> Source {
        self.source
    }

    /// The CLI's and its SDK's versions, as `version` prints them.
    pub async fn version(self) -> Result<String, Error> {
        let stdout = self.run(["version"]).await?;

        versions(&stdout).ok_or_else(|| Error::Parse(stdout.trim().to_owned()))
    }

    /// Who the session belongs to and how much My Files holds: the owner of
    /// its root folder, and that folder's size.
    pub async fn account(self) -> Result<Account, Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Root {
            owned_by: Owner,
        }

        #[derive(Deserialize)]
        struct Owner {
            email: Option<String>,
        }

        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Size {
            size: u64,
            number_of_descendants: u64,
        }

        let (root, size) = tokio::join!(
            self.run(["filesystem", "info", "--json", "/my-files"]),
            self.run(["filesystem", "size", "--json", "/my-files"]),
        );
        let root: Root = parse_json(&root?)?;

        // `filesystem size` is newer than some CLIs in use; the account is
        // shown without it.
        let usage = match size.and_then(|stdout| parse_json::<Size>(&stdout)) {
            Ok(size) => Some(Usage {
                bytes: size.size,
                items: size.number_of_descendants,
            }),
            Err(error) => {
                tracing::info!(%error, "no size for My Files");
                None
            }
        };

        Ok(Account {
            email: root.owned_by.email,
            usage,
        })
    }

    /// The contents of a folder, or of a section root such as `/devices`.
    pub async fn list(self, path: String) -> Result<Vec<Entry>, Error> {
        let stdout = self.run(["filesystem", "list", "--json", &path]).await?;
        let items: Vec<serde_json::Value> = parse_json(&stdout)?;

        let mut entries: Vec<Entry> = items
            .into_iter()
            .filter_map(|item| entry(item, &path))
            .collect();
        entries.sort_by_cached_key(|entry| (!entry.is_folder(), entry.name.to_lowercase()));

        Ok(entries)
    }

    /// Downloads a file into `dir` and returns where it landed.
    ///
    /// `dir` is the file's alone: whatever is in it is replaced.
    pub async fn download(self, path: String, dir: PathBuf) -> Result<PathBuf, Error> {
        tokio::fs::create_dir_all(&dir).await?;
        let dir_arg = dir.as_os_str().to_owned();
        self.run([
            OsString::from("filesystem"),
            "download".into(),
            "--json".into(),
            "--file-conflict-strategy".into(),
            "remove".into(),
            path.into(),
            dir_arg,
        ])
        .await?;

        // The CLI names the file itself, after the node; the directory holds
        // nothing else.
        let mut files = tokio::fs::read_dir(&dir).await?;
        while let Some(file) = files.next_entry().await? {
            if file.file_type().await?.is_file() {
                return Ok(file.path());
            }
        }

        Err(Error::Failed("The download finished without a file".into()))
    }

    /// Who the node at `path` is shared with, and its public link; `None`
    /// when it is not shared at all.
    pub async fn sharing(self, path: String) -> Result<Option<Sharing>, Error> {
        let stdout = self
            .run(["sharing", "status", "--json", "--", &path])
            .await?;
        parse_sharing(&stdout)
    }

    /// Invites `emails` to the node at `path` as `role`, and gives back
    /// the sharing as it then stands.
    pub async fn invite(
        self,
        path: String,
        emails: Vec<String>,
        role: Role,
    ) -> Result<Option<Sharing>, Error> {
        let mut args: Vec<OsString> = ["sharing", "invite", "--json", "--role", role.as_str()]
            .map(OsString::from)
            .into();
        for email in &emails {
            args.push("--user".into());
            args.push(email.into());
        }
        args.push("--".into());
        args.push(path.into());

        let stdout = self.run(args).await?;
        parse_sharing(&stdout)
    }

    /// Takes `emails`' access, or their pending invitations, away.
    pub async fn unshare(
        self,
        path: String,
        emails: Vec<String>,
    ) -> Result<Option<Sharing>, Error> {
        let mut args: Vec<OsString> = ["sharing", "remove", "--json"].map(OsString::from).into();
        for email in &emails {
            args.push("--email".into());
            args.push(email.into());
        }
        args.push("--".into());
        args.push(path.into());

        let stdout = self.run(args).await?;
        parse_sharing(&stdout)
    }

    /// Creates the node's public link, or sets its role.
    pub async fn set_link(self, path: String, role: Role) -> Result<Option<Sharing>, Error> {
        let stdout = self
            .run([
                "sharing",
                "set-url",
                "--json",
                "--role",
                role.as_str(),
                "--",
                &path,
            ])
            .await?;
        parse_sharing(&stdout)
    }

    /// Removes the node's public link; members keep their access.
    pub async fn remove_link(self, path: String) -> Result<Option<Sharing>, Error> {
        let stdout = self
            .run(["sharing", "remove-url", "--json", "--", &path])
            .await?;
        parse_sharing(&stdout)
    }

    /// Leaves a node shared with the account.
    pub async fn leave(self, path: String) -> Result<(), Error> {
        self.run(["sharing", "leave", "--json", "--", &path])
            .await
            .map(|_| ())
    }

    /// Moves `entries` to the trash.
    pub async fn trash(self, entries: Vec<Entry>) -> Result<Done, Error> {
        let paths = entries.iter().map(|entry| entry.path.clone()).collect();
        self.node_op("trash", paths, &entries).await
    }

    /// Restores `entries` from the trash.
    pub async fn restore(self, entries: Vec<Entry>) -> Result<Done, Error> {
        let paths = entries.iter().map(Entry::trash_path).collect();
        self.node_op("restore", paths, &entries).await
    }

    /// Deletes `entries`, which are in the trash, for good.
    pub async fn delete(self, entries: Vec<Entry>) -> Result<Done, Error> {
        let paths = entries.iter().map(Entry::trash_path).collect();
        self.node_op("delete", paths, &entries).await
    }

    /// Runs `filesystem <command>` over `paths`, and reads the result it
    /// prints for each node, named after `entries`.
    async fn node_op(
        &self,
        command: &str,
        paths: Vec<String>,
        entries: &[Entry],
    ) -> Result<Done, Error> {
        let mut args: Vec<OsString> = ["filesystem", command, "--json", "--"]
            .map(OsString::from)
            .into();
        args.extend(paths.into_iter().map(OsString::from));

        let stdout = self.run(args).await?;
        let results: Vec<NodeResult> = parse_json(&stdout)?;

        let name_of = |uid: &str| {
            entries
                .iter()
                .find(|entry| entry.uid == uid)
                .map_or_else(|| uid.to_owned(), |entry| entry.name.clone())
        };
        let mut done = Done::default();
        for result in results {
            if result.ok {
                done.done += 1;
            } else {
                let reason = result
                    .error
                    .as_ref()
                    .and_then(|error| error.get("message")?.as_str().map(str::to_owned));
                done.failed.push((name_of(&result.uid), reason));
            }
        }
        Ok(done)
    }

    /// Renames the node at `path` to `name`, where it is.
    pub async fn rename(self, path: String, name: String) -> Result<(), Error> {
        self.run(["filesystem", "rename", "--json", "--", &path, &name])
            .await
            .map(|_| ())
    }

    /// Creates a folder called `name` in the folder at `parent`.
    pub async fn create_folder(self, parent: String, name: String) -> Result<(), Error> {
        // After `--`, a name starting with `-` is not taken for an option.
        self.run([
            "filesystem",
            "create-folder",
            "--json",
            "--",
            &parent,
            &name,
        ])
        .await
        .map(|_| ())
    }

    /// Uploads `files` into the folder at `parent`. A file whose name is
    /// taken is uploaded under another, as "Keep Both" would; one whose
    /// content is already there is skipped by the CLI.
    pub async fn upload(self, files: Vec<PathBuf>, parent: String) -> Result<Transfer, Error> {
        let mut args: Vec<OsString> = [
            "filesystem",
            "upload",
            "--json",
            "--file-conflict-strategy",
            "rename",
            "--",
        ]
        .map(OsString::from)
        .into();
        args.extend(files.into_iter().map(OsString::from));
        args.push(parent.into());

        self.transfer(args).await
    }

    /// Downloads the nodes at `paths` into `dir`, folders with all they
    /// hold. What is already there by the same name is kept, the download
    /// taking another; Proton Docs and Sheets are skipped by the CLI.
    pub async fn download_to(self, paths: Vec<String>, dir: PathBuf) -> Result<Transfer, Error> {
        let mut args: Vec<OsString> = [
            "filesystem",
            "download",
            "--json",
            "--file-conflict-strategy",
            "rename",
            "--folder-conflict-strategy",
            "rename",
            "--",
        ]
        .map(OsString::from)
        .into();
        args.extend(paths.into_iter().map(OsString::from));
        args.push(dir.into());

        self.transfer(args).await
    }

    /// Runs a transfer and reads its summary — printed whether or not every
    /// item made it, as the exit status only says that one did not.
    async fn transfer(&self, args: Vec<OsString>) -> Result<Transfer, Error> {
        let (stdout, outcome) = self.execute(args).await?;
        match parse_json::<Summary>(&stdout) {
            Ok(summary) => Ok(summary.into()),
            Err(error) => outcome.and(Err(error)),
        }
    }

    /// Everything the CLI tells of the node at `path`.
    pub async fn info(self, path: String) -> Result<Details, Error> {
        let stdout = self
            .run(["filesystem", "info", "--json", "--", &path])
            .await?;

        parse_json::<NodeInfo>(&stdout).map(Details::from)
    }

    pub async fn logout(self) -> Result<(), Error> {
        self.run(["auth", "logout"]).await.map(|_| ())
    }

    /// Runs `auth login`: the sign-in page's address as soon as the CLI has
    /// it, then the outcome once the browser has finished. Dropping the
    /// stream kills the CLI.
    pub fn login(self) -> impl Stream<Item = Login> {
        iced::stream::channel(4, async move |mut output| {
            tracing::info!(program = %self.program.display(), "signing in");
            let result = async {
                let mut child = self
                    .command(["auth", "login", "--json"])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()?;

                let stdout = child
                    .stdout
                    .take()
                    .ok_or_else(|| Error::Io("no stdout".into()))?;
                let mut stderr = child
                    .stderr
                    .take()
                    .ok_or_else(|| Error::Io("no stderr".into()))?;
                let mut lines = BufReader::new(stdout).lines();

                while let Some(line) = lines.next_line().await? {
                    #[derive(Deserialize)]
                    #[serde(rename_all = "camelCase")]
                    struct SignIn {
                        sign_in_url: String,
                    }

                    if let Ok(SignIn { sign_in_url }) = serde_json::from_str(line.trim()) {
                        // The address carries a one-off sign-in token, so
                        // it is not logged.
                        tracing::info!("sign-in page ready");
                        let _ = output.send(Login::Url(sign_in_url)).await;
                    }
                }

                let mut errors = String::new();
                stderr.read_to_string(&mut errors).await?;
                let status = child.wait().await?;
                if status.success() {
                    Ok(())
                } else {
                    Err(failure(&errors, status))
                }
            }
            .await;

            match &result {
                Ok(()) => tracing::info!("signed in"),
                Err(error) => tracing::warn!(%error, "sign-in failed"),
            }
            let _ = output.send(Login::Done(result)).await;
        })
    }

    fn command<I, S>(&self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let mut command = Command::new(&self.program);
        command.args(args).stdin(Stdio::null()).kill_on_drop(true);
        command
    }

    async fn run<I, S>(&self, args: I) -> Result<String, Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let (stdout, outcome) = self.execute(args).await?;

        outcome.map(|()| stdout)
    }

    /// Runs the CLI: what it printed, and how it ended — for the commands
    /// that print a result even when they fail.
    async fn execute<I, S>(&self, args: I) -> Result<(String, Result<(), Error>), Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let mut command = self.command(args);
        let line = describe(&command);
        tracing::debug!(command = %line, "running");

        let started = Instant::now();
        let output = command.output().await.inspect_err(|error| {
            tracing::error!(command = %line, %error, "could not run the CLI");
        })?;
        let elapsed = started.elapsed();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr);

        if output.status.success() {
            tracing::info!(command = %line, ?elapsed, bytes = stdout.len(), "done");
            if !stderr.trim().is_empty() {
                tracing::debug!(command = %line, stderr = %stderr.trim(), "the CLI said");
            }
            Ok((stdout, Ok(())))
        } else {
            let said = if stderr.trim().is_empty() {
                &stdout
            } else {
                &*stderr
            };
            tracing::warn!(
                command = %line,
                ?elapsed,
                status = %output.status,
                output = %said.trim(),
                "failed",
            );
            let failure = failure(said, output.status);
            Ok((stdout, Err(failure)))
        }
    }
}

/// The versions in what `version` prints, as "CLI 0.8.0, SDK 0.21.0": each
/// line's `Proton Drive <part> <build>`, with the build's name before `@`
/// and its commit after `+` left out.
fn versions(stdout: &str) -> Option<String> {
    let versions: Vec<String> = stdout
        .lines()
        .filter_map(|line| line.trim().strip_prefix("Proton Drive "))
        .filter_map(|line| {
            let (part, build) = line.split_once(' ')?;
            let version = build.rsplit_once('@').map_or(build, |(_, version)| version);
            let version = version
                .split_once('+')
                .map_or(version, |(version, _)| version);
            Some(format!("{part} {version}"))
        })
        .collect();

    (!versions.is_empty()).then(|| versions.join(", "))
}

/// `command` as a shell would show it, for the log.
fn describe(command: &Command) -> String {
    let command = command.as_std();

    std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `name` as an executable file is named on this platform.
fn executable(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

/// What a failed command amounts to, from what it printed.
fn failure(said: &str, status: std::process::ExitStatus) -> Error {
    if said.contains(AUTH_REQUIRED) {
        return Error::AuthRequired;
    }

    // An uncaught error prints a banner of `=` and a stack trace; the
    // message is the first line that is neither.
    let message = said
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.chars().all(|c| c == '='))
        .map(|line| line.trim_start_matches("error: ").to_owned());

    Error::Failed(message.unwrap_or_else(|| format!("The CLI exited with {status}")))
}

/// Parses `stdout` as JSON, skipping anything printed before it.
fn parse_json<T: serde::de::DeserializeOwned>(stdout: &str) -> Result<T, Error> {
    let start = stdout.find(['[', '{']).unwrap_or(0);

    serde_json::from_str(&stdout[start..]).map_err(|error| Error::Parse(error.to_string()))
}

/// What a member of a share, or the public, may do — the SDK's
/// `MemberRole`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Viewer,
    Editor,
    Admin,
}

impl Role {
    pub const ALL: [Role; 3] = [Role::Viewer, Role::Editor, Role::Admin];

    /// The CLI's word for it.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Editor => "editor",
            Role::Admin => "admin",
        }
    }

    /// What it allows, as Proton Drive words it.
    pub fn label(self) -> &'static str {
        match self {
            Role::Viewer => "Viewer",
            Role::Editor => "Editor",
            Role::Admin => "Admin",
        }
    }

    fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|role| role.as_str() == word)
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// A node's sharing — the SDK's `ShareResult`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sharing {
    pub members: Vec<Member>,
    pub link: Option<Link>,
}

/// Someone a node is shared with, or invited to; `pending` until they
/// accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub email: String,
    pub role: Role,
    pub pending: bool,
}

/// A node's public link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub url: String,
    pub role: Role,
    pub has_password: bool,
    /// RFC 3339.
    pub expires: Option<String>,
    pub downloads: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShareResult {
    #[serde(default)]
    members: Vec<MemberJson>,
    #[serde(default)]
    proton_invitations: Vec<MemberJson>,
    #[serde(default)]
    non_proton_invitations: Vec<MemberJson>,
    url_access: Option<UrlAccess>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MemberJson {
    invitee_email: String,
    role: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UrlAccess {
    url: String,
    role: String,
    custom_password: Option<String>,
    expiration_time: Option<String>,
    #[serde(default)]
    number_of_initialized_downloads: u64,
}

/// The sharing a `sharing` command prints: `undefined` for a node that is
/// not shared, else the SDK's `ShareResult`.
fn parse_sharing(stdout: &str) -> Result<Option<Sharing>, Error> {
    let text = stdout.trim();
    if text.is_empty() || text == "undefined" || text == "null" {
        return Ok(None);
    }

    let result: ShareResult = parse_json(text)?;
    let member = |pending: bool| {
        move |member: MemberJson| Member {
            email: member.invitee_email,
            role: Role::parse(&member.role).unwrap_or(Role::Viewer),
            pending,
        }
    };
    let members = result
        .members
        .into_iter()
        .map(member(false))
        .chain(result.proton_invitations.into_iter().map(member(true)))
        .chain(result.non_proton_invitations.into_iter().map(member(true)))
        .collect();
    let link = result.url_access.map(|access| Link {
        url: access.url,
        role: Role::parse(&access.role).unwrap_or(Role::Viewer),
        has_password: access
            .custom_password
            .is_some_and(|password| !password.is_empty()),
        expires: access.expiration_time,
        downloads: access.number_of_initialized_downloads,
    });

    Ok(Some(Sharing { members, link }))
}

/// How an operation over nodes went: how many it took, and which it did
/// not, with why when the CLI says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Done {
    pub done: usize,
    pub failed: Vec<(String, Option<String>)>,
}

/// What `trash`, `restore` and `delete` print for each node.
#[derive(Debug, Deserialize)]
struct NodeResult {
    uid: String,
    ok: bool,
    error: Option<serde_json::Value>,
}

/// How an upload or a download went.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Transfer {
    pub transferred: u64,
    /// Files whose content was already there.
    pub skipped: u64,
    /// Each file that failed, with why when the CLI says.
    pub failures: Vec<(String, Option<String>)>,
}

/// A node's metadata, as the Info dialog shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Details {
    pub media_type: Option<String>,
    /// RFC 3339.
    pub created: Option<String>,
    pub modified: Option<String>,
    /// Who made the node, as its key's signature says; `None` when it
    /// cannot be verified.
    pub created_by: Option<String>,
    pub owner: Option<String>,
    pub size: Option<u64>,
    /// What every revision takes on the server, encrypted.
    pub stored: Option<u64>,
    pub shared: bool,
    pub shared_by_link: bool,
    pub sha1: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NodeInfo {
    media_type: Option<String>,
    creation_time: Option<String>,
    modification_time: Option<String>,
    key_author: Option<serde_json::Value>,
    #[serde(default)]
    owned_by: Option<Owner>,
    total_storage_size: Option<u64>,
    #[serde(default)]
    is_shared: bool,
    #[serde(default)]
    is_shared_by_url: bool,
    active_revision: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct Owner {
    email: Option<String>,
}

impl From<NodeInfo> for Details {
    fn from(node: NodeInfo) -> Self {
        // The revision, bare or, from CLIs before 0.7, in a `Result`.
        let revision = node
            .active_revision
            .map(|revision| match revision.get("value") {
                Some(value) if revision.get("ok").is_some() => value.clone(),
                _ => revision,
            });
        let revision = revision.as_ref();

        // `Result<string | AnonymousUser, …>`: an address when verified.
        let created_by = node
            .key_author
            .as_ref()
            .filter(|author| author.get("ok").and_then(|ok| ok.as_bool()) == Some(true))
            .and_then(|author| author.get("value")?.as_str())
            .map(str::to_owned);

        let text = |value: Option<&serde_json::Value>| {
            value.and_then(|value| value.as_str()).map(str::to_owned)
        };

        Details {
            media_type: node.media_type,
            created: node.creation_time,
            modified: text(revision.and_then(|revision| revision.get("claimedModificationTime")))
                .or(node.modification_time),
            created_by,
            owner: node.owned_by.and_then(|owner| owner.email),
            size: revision
                .and_then(|revision| revision.get("claimedSize"))
                .and_then(|size| size.as_u64()),
            stored: node.total_storage_size,
            shared: node.is_shared,
            shared_by_link: node.is_shared_by_url,
            sha1: text(
                revision
                    .and_then(|revision| revision.get("claimedDigests"))
                    .and_then(|digests| digests.get("sha1")),
            ),
        }
    }
}

/// What a transfer prints with `--json` once it is over.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Summary {
    transferred_items: u64,
    skipped_items: u64,
    #[serde(default)]
    failures: Vec<SummaryFailure>,
}

#[derive(Debug, Deserialize)]
struct SummaryFailure {
    name: String,
    /// A message from newer CLIs; 0.8 prints the error object itself,
    /// which is `{}` once in JSON.
    error: Option<serde_json::Value>,
}

impl From<Summary> for Transfer {
    fn from(summary: Summary) -> Self {
        let failures = summary
            .failures
            .into_iter()
            .map(|failure| {
                let reason = match failure.error {
                    Some(serde_json::Value::String(message)) => Some(message),
                    Some(error) => error
                        .get("message")
                        .and_then(|message| message.as_str())
                        .map(str::to_owned),
                    None => None,
                };
                (failure.name, reason.filter(|reason| !reason.is_empty()))
            })
            .collect();

        Self {
            transferred: summary.transferred_items,
            skipped: summary.skipped_items,
            failures,
        }
    }
}

/// The SDK's `Result<string, Error | InvalidNameError>`: an undecryptable
/// name may still carry a placeholder to show.
#[derive(Debug, Deserialize)]
struct Name {
    ok: bool,
    value: Option<String>,
    error: Option<serde_json::Value>,
}

impl Name {
    fn display(&self) -> Option<String> {
        if self.ok {
            return self.value.clone().filter(|name| !name.is_empty());
        }

        self.error
            .as_ref()?
            .get("name")?
            .as_str()
            .map(str::to_owned)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Node {
    uid: String,
    name: Name,
    #[serde(rename = "type")]
    kind: String,
    media_type: Option<String>,
    #[serde(default)]
    is_shared: bool,
    modification_time: Option<String>,
    active_revision: Option<ActiveRevision>,
    folder: Option<Folder>,
}

/// CLIs before 0.7 wrap the active revision in a `Result`, as `{"ok": true,
/// "value": …}`; later ones give it bare.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ActiveRevision {
    /// `ok` is what tells the wrapper from a bare revision; `value` is
    /// absent when it is `false`.
    Wrapped {
        #[serde(rename = "ok")]
        _ok: bool,
        value: Option<Revision>,
    },
    Bare(Revision),
}

impl ActiveRevision {
    fn into_revision(self) -> Option<Revision> {
        match self {
            ActiveRevision::Wrapped { value, .. } => value,
            ActiveRevision::Bare(revision) => Some(revision),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Revision {
    uid: String,
    claimed_size: Option<u64>,
    claimed_modification_time: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Folder {
    claimed_modification_time: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Device {
    uid: String,
    name: Name,
    root_folder_uid: String,
    last_sync_time: Option<String>,
}

/// One item of a listing, in whichever shape it came: a node, a device, or
/// a section of the root (which the sidebar already shows, so it is dropped).
fn entry(item: serde_json::Value, parent: &str) -> Option<Entry> {
    if item.get("rootFolderUid").is_some() {
        let device: Device = serde_json::from_value(item).ok()?;
        let name = device.name.display().unwrap_or_else(|| device.uid.clone());
        let path = node_path(&device.root_folder_uid, parent, &name);

        return Some(Entry {
            uid: device.root_folder_uid,
            name,
            kind: Kind::Device,
            media_type: None,
            size: None,
            modified: device.last_sync_time,
            shared: false,
            revision: None,
            path,
        });
    }

    let node: Node = serde_json::from_value(item).ok()?;
    let name = node.name.display().unwrap_or_else(|| node.uid.clone());
    let path = node_path(&node.uid, parent, &name);
    let kind = match node.kind.as_str() {
        "folder" | "album" => Kind::Folder,
        _ => Kind::File,
    };

    let (size, revision, claimed) =
        match node.active_revision.and_then(ActiveRevision::into_revision) {
            Some(revision) => (
                revision.claimed_size,
                Some(revision.uid),
                revision.claimed_modification_time,
            ),
            None => (None, None, None),
        };
    let claimed = claimed.or(node
        .folder
        .and_then(|folder| folder.claimed_modification_time));

    Some(Entry {
        uid: node.uid,
        name,
        kind,
        media_type: node.media_type,
        size,
        modified: claimed.or(node.modification_time),
        shared: node.is_shared,
        revision,
        path,
    })
}

/// How the CLI is to find a node: by its UID when it has the shape the CLI
/// recognises, else by name under its parent.
fn node_path(uid: &str, parent: &str, name: &str) -> String {
    // `restore` and `delete` take trashed items by name under `/trash`
    // only, so that is how the trash's items are addressed.
    if parent.trim_end_matches('/') == "/trash" {
        return trash_path(name);
    }
    if is_node_uid(uid) {
        format!("/my-files/{uid}")
    } else {
        format!(
            "{}/{}",
            parent.trim_end_matches('/'),
            name.replace('/', "\\/")
        )
    }
}

/// Where a trashed item is addressed: `/trash/<name>`, `/` in the name
/// escaped as the CLI reads it.
fn trash_path(name: &str) -> String {
    format!("/trash/{}", name.replace('/', "\\/"))
}

/// The CLI's `isNodeUid`: two IDs joined by `~`, each 22 URL-safe base64
/// characters or 88 to 108 of base64.
fn is_node_uid(uid: &str) -> bool {
    fn part(id: &str) -> bool {
        let short = id.len() == 22
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        let long = (88..=108).contains(&id.len())
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'=' | b'_' | b'-'));
        short || long
    }

    uid.split_once('~')
        .is_some_and(|(volume, node)| part(volume) && part(node))
}

#[cfg(test)]
mod tests {
    use super::*;

    const UID: &str = "AAAAAAAAAAAAAAAAAAAAAA~BBBBBBBBBBBBBBBBBBBBBB";

    #[test]
    fn nodes_are_parsed_and_addressed_by_uid() {
        let json = format!(
            r#"[
{{"uid":"{UID}","name":{{"ok":true,"value":"notes.txt"}},"type":"file","mediaType":"text/plain","isShared":true,"isSharedByUrl":false,"creationTime":"2026-01-01T00:00:00.000Z","modificationTime":"2026-02-01T00:00:00.000Z","activeRevision":{{"uid":"rev","claimedSize":42,"claimedModificationTime":"2026-03-01T00:00:00.000Z"}}}},
{{"uid":"x~y","name":{{"ok":false,"error":{{"name":"Bad\u0000name","error":"invalid"}}}},"type":"folder"}}
]"#
        );

        let mut entries: Vec<Entry> = parse_json::<Vec<serde_json::Value>>(&json)
            .unwrap()
            .into_iter()
            .filter_map(|item| entry(item, "/my-files/some"))
            .collect();
        entries.sort_by_key(|entry| !entry.is_folder());

        assert_eq!(entries[0].kind, Kind::Folder);
        assert_eq!(entries[0].path, "/my-files/some/Bad\0name");
        assert_eq!(entries[1].name, "notes.txt");
        assert_eq!(entries[1].path, format!("/my-files/{UID}"));
        assert_eq!(entries[1].size, Some(42));
        assert_eq!(
            entries[1].modified.as_deref(),
            Some("2026-03-01T00:00:00.000Z")
        );
        assert!(entries[1].shared);
    }

    #[test]
    fn revisions_wrapped_by_older_clis_are_unwrapped() {
        let item = serde_json::json!({
            "uid": UID,
            "name": {"ok": true, "value": "old.txt"},
            "type": "file",
            "activeRevision": {"ok": true, "value": {"uid": "rev", "claimedSize": 7}},
        });
        let entry = entry(item, "/my-files").unwrap();

        assert_eq!(entry.size, Some(7));
        assert_eq!(entry.revision.as_deref(), Some("rev"));
    }

    #[test]
    fn devices_list_as_their_root_folders() {
        let json = format!(
            r#"[{{"uid":"dev","type":"Linux","name":{{"ok":true,"value":"Laptop"}},"rootFolderUid":"{UID}","creationTime":"2026-01-01T00:00:00.000Z","shareId":"s"}}]"#
        );
        let items: Vec<serde_json::Value> = parse_json(&json).unwrap();
        let device = entry(items.into_iter().next().unwrap(), "/devices").unwrap();

        assert_eq!(device.kind, Kind::Device);
        assert_eq!(device.name, "Laptop");
        assert_eq!(device.path, format!("/my-files/{UID}"));
    }

    #[test]
    fn root_sections_are_dropped() {
        let items: Vec<serde_json::Value> = parse_json(r#"[{"path":"/my-files"}]"#).unwrap();
        assert!(
            items
                .into_iter()
                .filter_map(|item| entry(item, "/"))
                .next()
                .is_none()
        );
    }

    #[test]
    fn output_before_the_json_is_skipped() {
        let items: Vec<serde_json::Value> = parse_json("Updating cache…\n[\n]\n").unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn failures_are_read_from_stderr() {
        let status = std::process::Command::new("false").status().unwrap();

        assert!(matches!(
            failure("You need to login first\n", status),
            Error::AuthRequired
        ));
        assert!(matches!(
            failure("=====\nValidationError: Node not found: x\n    at foo", status),
            Error::Failed(message) if message == "ValidationError: Node not found: x"
        ));
    }

    #[test]
    fn versions_are_read_from_what_version_prints() {
        let stdout = "Proton Drive CLI cli-drive@0.8.0+06e8c605\n\
                      Proton Drive SDK js@0.21.0+06e8c605\n\
                      You are running the latest version.\n";

        assert_eq!(versions(stdout).as_deref(), Some("CLI 0.8.0, SDK 0.21.0"));
        assert_eq!(
            versions("Proton Drive CLI 0.4.1").as_deref(),
            Some("CLI 0.4.1")
        );
        assert_eq!(versions("unexpected"), None);
    }

    #[test]
    fn upload_summaries_are_read_from_either_version() {
        let current = r#"{"transferredItems":2,"transferredBytes":10,"skippedItems":1,"failedItems":1,"failures":[{"name":"a.txt","error":"ValidationError: Too big"}]}"#;
        let release = r#"{"transferredItems":0,"transferredBytes":0,"skippedItems":0,"failedItems":1,"failures":[{"name":"b.txt","nodeUid":"x","error":{}}]}"#;

        let current: Transfer = parse_json::<Summary>(current).unwrap().into();
        let release: Transfer = parse_json::<Summary>(release).unwrap().into();

        assert_eq!(current.transferred, 2);
        assert_eq!(current.skipped, 1);
        assert_eq!(
            current.failures,
            [(
                "a.txt".to_owned(),
                Some("ValidationError: Too big".to_owned())
            )]
        );
        assert_eq!(release.failures, [("b.txt".to_owned(), None)]);
    }

    #[test]
    fn node_info_is_read_into_details() {
        let json = r#"{"uid":"x","name":{"ok":true,"value":"a.txt"},"type":"file","mediaType":"text/plain",
            "keyAuthor":{"ok":true,"value":"ada@proton.me"},"ownedBy":{"email":"ada@proton.me"},
            "creationTime":"2026-01-01T00:00:00.000Z","modificationTime":"2026-02-01T00:00:00.000Z",
            "isShared":true,"isSharedByUrl":true,"totalStorageSize":2048,
            "activeRevision":{"uid":"r","claimedSize":1000,"claimedDigests":{"sha1":"abc","sha1Verified":true}}}"#;
        let details = Details::from(parse_json::<NodeInfo>(json).unwrap());

        assert_eq!(details.created_by.as_deref(), Some("ada@proton.me"));
        assert_eq!(details.size, Some(1000));
        assert_eq!(details.stored, Some(2048));
        assert_eq!(details.sha1.as_deref(), Some("abc"));
        assert_eq!(
            details.modified.as_deref(),
            Some("2026-02-01T00:00:00.000Z")
        );
        assert!(details.shared_by_link);

        // An author that cannot be verified is not named.
        let json = r#"{"keyAuthor":{"ok":false,"error":{"claimedAuthor":"eve@x"}},"activeRevision":{"ok":true,"value":{"claimedSize":5}}}"#;
        let details = Details::from(parse_json::<NodeInfo>(json).unwrap());
        assert_eq!(details.created_by, None);
        assert_eq!(details.size, Some(5));
    }

    #[test]
    fn sharing_is_read_from_status() {
        assert_eq!(parse_sharing("undefined\n").unwrap(), None);

        let json = r#"{"protonInvitations":[{"uid":"i","inviteeEmail":"bob@proton.me","role":"editor","invitationTime":"2026-01-01T00:00:00.000Z","addedByEmail":{"ok":true,"value":"me@proton.me"}}],
            "nonProtonInvitations":[],"members":[{"uid":"m","inviteeEmail":"ann@proton.me","role":"viewer","invitationTime":"2026-01-01T00:00:00.000Z","addedByEmail":{"ok":true,"value":"me@proton.me"}}],
            "urlAccess":{"uid":"u","creationTime":"2026-01-01T00:00:00.000Z","role":"viewer","url":"https://drive.proton.me/urls/x#y","numberOfInitializedDownloads":3},"editorsCanShare":false}"#;
        let sharing = parse_sharing(json).unwrap().unwrap();

        assert_eq!(sharing.members.len(), 2);
        assert_eq!(sharing.members[0].email, "ann@proton.me");
        assert!(!sharing.members[0].pending);
        assert_eq!(sharing.members[1].role, Role::Editor);
        assert!(sharing.members[1].pending);
        let link = sharing.link.unwrap();
        assert_eq!(link.url, "https://drive.proton.me/urls/x#y");
        assert!(!link.has_password);
        assert_eq!(link.downloads, 3);
    }

    #[test]
    fn trashed_items_are_addressed_by_name() {
        let item = serde_json::json!({"uid": UID, "name": {"ok": true, "value": "a/b.txt"}, "type": "file"});
        let entry = entry(item, "/trash").unwrap();

        assert_eq!(entry.path, "/trash/a\\/b.txt");
        assert_eq!(entry.trash_path(), "/trash/a\\/b.txt");
    }

    #[test]
    fn uids_are_recognised_as_the_cli_does() {
        assert!(is_node_uid(UID));
        assert!(!is_node_uid("x~y"));
        assert!(!is_node_uid("notes.txt"));
    }
}
