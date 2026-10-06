//! What becomes of a file once it is opened: shown in the window when it is
//! text or an image, handed to the desktop's default application otherwise.

use std::path::{Path, PathBuf};

use crate::drive::{Entry, Error};

/// Text past this size goes to an external application: iced's editor lays
/// out the whole buffer at once.
pub const MAX_TEXT_BYTES: u64 = 4 * 1024 * 1024;

/// How much of a file of unknown type is read to tell text from binary.
const SNIFF_BYTES: usize = 8 * 1024;

/// What a file is shown as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Text,
    Image,
    Svg,
    /// A Proton Docs document or Proton Sheets spreadsheet: no file, but a
    /// page of docs.proton.me, which the CLI does not download.
    Document(Document),
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Document {
    Doc,
    Sheet,
}

impl Document {
    /// The SDK's `isProtonDocument` and `isProtonSheet`.
    fn of(media_type: &str) -> Option<Self> {
        match media_type {
            "application/vnd.proton.doc" => Some(Self::Doc),
            "application/vnd.proton.sheet" => Some(Self::Sheet),
            _ => None,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Doc => "Proton Docs document",
            Self::Sheet => "Proton Sheets spreadsheet",
        }
    }
}

/// Where a Proton Docs document or Sheets spreadsheet opens, as the SDK's
/// `getNodeUrl` makes it from the node's volume and id.
pub fn document_url(entry: &Entry) -> Option<String> {
    let document = Document::of(entry.media_type.as_deref()?)?;
    let (volume, node) = entry.uid.split_once('~')?;
    let kind = match document {
        Document::Doc => "doc",
        Document::Sheet => "sheet",
    };

    Some(format!(
        "https://docs.proton.me/doc?type={kind}&mode=open&volumeId={volume}&linkId={node}"
    ))
}

/// A downloaded file, ready to show.
#[derive(Debug, Clone)]
pub struct Opened {
    pub path: PathBuf,
    pub content: Content,
}

#[derive(Debug, Clone)]
pub enum Content {
    Text(String),
    Image,
    Svg,
    /// Handed to another application.
    External,
}

/// What `entry` most likely is, from its media type and its name.
pub fn classify(entry: &Entry) -> Class {
    let media = entry.media_type.as_deref().unwrap_or_default();
    if let Some(document) = Document::of(media) {
        return Class::Document(document);
    }
    let extension = Path::new(&entry.name)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    if media == "image/svg+xml" || extension == "svg" {
        Class::Svg
    } else if RASTER_TYPES.contains(&media) || RASTER_EXTENSIONS.contains(&extension.as_str()) {
        Class::Image
    } else if media.starts_with("text/")
        || TEXT_TYPES.contains(&media)
        || media.ends_with("+json")
        || media.ends_with("+xml")
        || TEXT_EXTENSIONS.contains(&extension.as_str())
    {
        Class::Text
    } else {
        Class::Other
    }
}

/// The formats the `image` crate decodes.
const RASTER_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/bmp",
    "image/x-bmp",
    "image/tiff",
    "image/x-icon",
    "image/vnd.microsoft.icon",
    "image/x-portable-anymap",
    "image/x-tga",
    "image/x-exr",
    "image/qoi",
];

const RASTER_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "jpe", "gif", "webp", "bmp", "tif", "tiff", "ico", "pnm", "pbm", "pgm",
    "ppm", "tga", "exr", "qoi",
];

const TEXT_TYPES: &[&str] = &[
    "application/json",
    "application/xml",
    "application/javascript",
    "application/x-sh",
    "application/x-shellscript",
    "application/toml",
    "application/x-yaml",
    "application/yaml",
    "application/sql",
    "application/x-subrip",
    "application/x-desktop",
];

const TEXT_EXTENSIONS: &[&str] = &[
    "txt",
    "md",
    "markdown",
    "rst",
    "org",
    "adoc",
    "log",
    "csv",
    "tsv",
    "json",
    "jsonc",
    "xml",
    "yaml",
    "yml",
    "toml",
    "ini",
    "cfg",
    "conf",
    "env",
    "properties",
    "rs",
    "c",
    "h",
    "cc",
    "cpp",
    "hpp",
    "py",
    "rb",
    "go",
    "java",
    "kt",
    "swift",
    "js",
    "mjs",
    "ts",
    "tsx",
    "jsx",
    "css",
    "scss",
    "html",
    "htm",
    "sh",
    "bash",
    "zsh",
    "fish",
    "lua",
    "sql",
    "tex",
    "srt",
    "vtt",
    "diff",
    "patch",
    "gitignore",
    "dockerfile",
    "makefile",
];

/// Where a downloaded copy of `entry` is kept: one directory per revision,
/// so a file opened twice is downloaded once, and a new version afresh.
///
/// The directory is named by a hash: a revision's UID joins three IDs of up
/// to 108 characters each, past what a file name may be.
pub fn cache_dir(entry: &Entry) -> PathBuf {
    let revision = entry.revision.as_deref().unwrap_or_default();

    cache_dir_root().join(format!(
        "{:016x}{:016x}",
        fnv1a(&entry.uid),
        fnv1a(revision)
    ))
}

/// Where every downloaded copy is kept.
pub fn cache_dir_root() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("cold-drive")
        .join("files")
}

/// 64-bit FNV-1a: stable across builds, unlike `std`'s hasher.
fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The file already downloaded into `dir`, if there is one.
pub async fn cached(dir: &Path) -> Option<PathBuf> {
    let mut files = tokio::fs::read_dir(dir).await.ok()?;
    while let Ok(Some(file)) = files.next_entry().await {
        if file.file_type().await.is_ok_and(|kind| kind.is_file()) {
            return Some(file.path());
        }
    }
    None
}

/// Reads the downloaded file as what `class` says it is — falling back to
/// another application when it is too big, or not text after all.
pub async fn load(path: PathBuf, class: Class) -> Result<Opened, Error> {
    let content = match class {
        Class::Image => Content::Image,
        Class::Svg => Content::Svg,
        // Opened on docs.proton.me before anything is downloaded.
        Class::Document(_) => Content::External,
        Class::Text | Class::Other => {
            let size = tokio::fs::metadata(&path).await?.len();
            if size > MAX_TEXT_BYTES {
                Content::External
            } else {
                let bytes = tokio::fs::read(&path).await?;
                match text(bytes, class == Class::Text) {
                    Some(text) => Content::Text(text),
                    None => Content::External,
                }
            }
        }
    };

    Ok(Opened { path, content })
}

/// `bytes` as text, if they are. A file not known to be text is taken for
/// text only if its start has no NUL and decodes as UTF-8, as GLib guesses.
fn text(bytes: Vec<u8>, known: bool) -> Option<String> {
    if !known {
        let head = &bytes[..bytes.len().min(SNIFF_BYTES)];
        if head.contains(&0) {
            return None;
        }
        // A cut through a multi-byte character at the end is still text.
        if let Err(error) = std::str::from_utf8(head)
            && error.error_len().is_some()
        {
            return None;
        }
    }

    let bytes = bytes
        .strip_prefix(b"\xEF\xBB\xBF".as_slice())
        .map(<[u8]>::to_vec)
        .unwrap_or(bytes);

    match String::from_utf8(bytes) {
        Ok(text) => Some(text),
        Err(error) if known => Some(String::from_utf8_lossy(error.as_bytes()).into_owned()),
        Err(_) => None,
    }
}

/// Opens `target` — a file or a URL — with the desktop's default handler.
pub async fn open_externally(target: String) -> Result<(), String> {
    let mut command = if cfg!(target_os = "macos") {
        tokio::process::Command::new("open")
    } else if cfg!(windows) {
        let mut command = tokio::process::Command::new("cmd");
        command.args(["/C", "start", ""]);
        command
    } else {
        tokio::process::Command::new("xdg-open")
    };

    let output = command
        .arg(&target)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|error| format!("Could not open {target}: {error}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let said = String::from_utf8_lossy(&output.stderr);
        let said = said.trim();
        Err(if said.is_empty() {
            "No application is available to open this file".to_owned()
        } else {
            said.to_owned()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drive::Kind;

    fn file(name: &str, media_type: Option<&str>) -> Entry {
        Entry {
            uid: "u".into(),
            name: name.into(),
            kind: Kind::File,
            media_type: media_type.map(Into::into),
            size: None,
            modified: None,
            shared: false,
            revision: Some("rev~1".into()),
            path: String::new(),
        }
    }

    #[test]
    fn files_are_classified_by_type_then_name() {
        assert_eq!(classify(&file("a.bin", Some("text/plain"))), Class::Text);
        assert_eq!(classify(&file("a.JPG", None)), Class::Image);
        assert_eq!(classify(&file("a", Some("image/png"))), Class::Image);
        assert_eq!(
            classify(&file("logo.svg", Some("application/octet-stream"))),
            Class::Svg
        );
        assert_eq!(
            classify(&file("a.json", Some("application/octet-stream"))),
            Class::Text
        );
        assert_eq!(
            classify(&file("a.pdf", Some("application/pdf"))),
            Class::Other
        );
        assert_eq!(classify(&file("a.heic", Some("image/heic"))), Class::Other);
    }

    #[test]
    fn proton_documents_open_on_docs_proton_me() {
        let mut doc = file("Test doc", Some("application/vnd.proton.doc"));
        doc.uid = "vol==~node==".into();

        assert_eq!(classify(&doc), Class::Document(Document::Doc));
        assert_eq!(
            document_url(&doc).as_deref(),
            Some("https://docs.proton.me/doc?type=doc&mode=open&volumeId=vol==&linkId=node==")
        );
        assert_eq!(document_url(&file("a.txt", Some("text/plain"))), None);
    }

    #[test]
    fn unknown_files_are_sniffed_for_text() {
        assert_eq!(text(b"hello".to_vec(), false).as_deref(), Some("hello"));
        assert_eq!(
            text(b"\xEF\xBB\xBFbom".to_vec(), false).as_deref(),
            Some("bom")
        );
        assert_eq!(text(b"a\0b".to_vec(), false), None);
        assert_eq!(text(b"\xff\xfe".to_vec(), false), None);
        assert_eq!(
            text(b"\xff ok".to_vec(), true).as_deref(),
            Some("\u{FFFD} ok")
        );
    }

    #[test]
    fn cache_dirs_are_keyed_by_revision() {
        let long = "A".repeat(108);
        let mut entry = file("a.txt", None);
        entry.revision = Some(format!("{long}~{long}~{long}"));
        let first = cache_dir(&entry);

        entry.revision = Some("rev~2".into());
        let second = cache_dir(&entry);

        assert_ne!(first, second);
        assert_eq!(first.file_name().unwrap().len(), 32);
    }
}
