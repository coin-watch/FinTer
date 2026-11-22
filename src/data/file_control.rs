//! Agent-driven workspace file editing: the wire format for `file` directives.
//!
//! The Agent inspects and edits local text files by emitting fenced `file`
//! blocks. Reading is a two-step exchange: the reply requests paths, the app
//! reads them, and a follow-up request carries the contents back to the model.
//! Writes are applied immediately and reported as receipts, following the same
//! pattern as `trade`, `ui`, and `terminal` directives.
//!
//! This module is deliberately free of UI types so the data layer never imports
//! `crate::components`.

use serde::{Deserialize, Serialize};

/// Maximum bytes a single `write`/`append` block may carry.
pub const MAX_FILE_WRITE_BYTES: usize = 2 * 1024 * 1024;
/// Maximum number of paths one `read` action may request.
pub const MAX_READ_FILES: usize = 5;
/// Maximum bytes of one file returned to the model.
pub const MAX_READ_BYTES: usize = 64 * 1024;

/// A single workspace file command emitted by the Agent as a fenced `file`
/// block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum FileAction {
    /// Ask the app to return the contents of one or more workspace files.
    Read {
        #[serde(default)]
        paths: Vec<String>,
    },
    /// Replace a file with `content`, creating it (and missing parent folders)
    /// when it does not exist.
    Write {
        #[serde(default)]
        path: Option<String>,
        content: String,
        /// Overwrite an open editor that has unsaved changes.
        #[serde(default)]
        force: bool,
    },
    /// Append `content` to a file, creating it when it does not exist.
    Append {
        #[serde(default)]
        path: Option<String>,
        content: String,
        #[serde(default)]
        force: bool,
    },
}

impl FileAction {
    /// Wire name of the action, used for receipts and error reporting.
    pub fn action_name(&self) -> &'static str {
        match self {
            Self::Read { .. } => "read",
            Self::Write { .. } => "write",
            Self::Append { .. } => "append",
        }
    }

    /// Explicit target path, if the action supplied one.
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::Read { .. } => None,
            Self::Write { path, .. } | Self::Append { path, .. } => path.as_deref(),
        }
    }

    pub fn content(&self) -> Option<&str> {
        match self {
            Self::Read { .. } => None,
            Self::Write { content, .. } | Self::Append { content, .. } => Some(content),
        }
    }

    pub fn force(&self) -> bool {
        match self {
            Self::Read { .. } => false,
            Self::Write { force, .. } | Self::Append { force, .. } => *force,
        }
    }
}

/// One parsed `file` fence: either a usable action or the reason it was
/// rejected.
#[derive(Clone, Debug, PartialEq)]
pub enum ParsedFileAction {
    Action(FileAction),
    Invalid(String),
}

/// Parses every complete fenced `file` block in `text`, in order.
///
/// Unclosed fences are ignored, matching the trade/chart/terminal parsers.
pub fn parse_file_actions_in_text(text: &str) -> Vec<ParsedFileAction> {
    let mut parsed = Vec::new();
    let mut lines = text.split_inclusive('\n');
    while let Some(line) = lines.next() {
        if !is_file_fence_opener(line) {
            continue;
        }
        let mut body = String::new();
        let mut closed = false;
        for inner in lines.by_ref() {
            if inner.trim_start().starts_with("```") {
                closed = true;
                break;
            }
            body.push_str(inner);
        }
        if !closed {
            continue;
        }
        match serde_json::from_str::<FileAction>(body.trim()) {
            Ok(action) => parsed.push(ParsedFileAction::Action(action)),
            Err(error) => parsed.push(ParsedFileAction::Invalid(format!(
                "malformed file directive: {error}"
            ))),
        }
    }
    parsed
}

/// True when `line` opens a `file` directive fence.
pub fn is_file_fence_opener(line: &str) -> bool {
    let Some(rest) = line.trim_start().strip_prefix("```") else {
        return false;
    };
    let language = rest.trim().to_ascii_lowercase();
    let language = language.split_whitespace().next().unwrap_or("");
    matches!(language, "file" | "finter-file")
}

/// True when a workspace path uses the layout-file suffix.
///
/// Checked by name as well as by parsing, so the Agent cannot rewrite a
/// malformed layout as if it were ordinary text.
pub fn is_layout_path(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".finter.json")
}

/// True when a workspace path looks like UTF-8 text the editor can show.
///
/// Extensions the file viewer treats as images or binaries are rejected so the
/// Agent cannot corrupt them with text writes.
pub fn is_text_file_path(path: &str) -> bool {
    let extension = std::path::Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase());
    !matches!(
        extension.as_deref(),
        Some(
            "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "webp"
                | "bmp"
                | "ico"
                | "svg"
                | "pdf"
                | "zip"
                | "gz"
                | "tar"
                | "rar"
                | "7z"
                | "exe"
                | "dll"
                | "so"
                | "dylib"
                | "bin"
                | "mp3"
                | "wav"
                | "flac"
                | "mp4"
                | "mov"
                | "avi"
                | "mkv"
                | "woff"
                | "woff2"
                | "ttf"
                | "otf"
                | "db"
                | "sqlite"
                | "parquet"
                | "arrow"
                | "wasm"
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_read_write_and_append() {
        let text = r#"```file
{"action":"read","paths":["README.md","notes/a.txt"]}
```
```file
{"action":"write","path":"notes/a.txt","content":"hello\nworld"}
```
```file
{"action":"append","content":"\nmore"}
```"#;
        let parsed = parse_file_actions_in_text(text);
        assert_eq!(parsed.len(), 3);
        match &parsed[0] {
            ParsedFileAction::Action(FileAction::Read { paths }) => {
                assert_eq!(
                    paths,
                    &vec!["README.md".to_string(), "notes/a.txt".to_string()]
                );
            }
            other => panic!("expected read, got {other:?}"),
        }
        match &parsed[1] {
            ParsedFileAction::Action(FileAction::Write {
                path,
                content,
                force,
            }) => {
                assert_eq!(path.as_deref(), Some("notes/a.txt"));
                assert_eq!(content, "hello\nworld");
                assert!(!force);
            }
            other => panic!("expected write, got {other:?}"),
        }
        match &parsed[2] {
            ParsedFileAction::Action(FileAction::Append { path, content, .. }) => {
                assert_eq!(path.as_deref(), None);
                assert_eq!(content, "\nmore");
            }
            other => panic!("expected append, got {other:?}"),
        }
    }

    #[test]
    fn reports_malformed_blocks() {
        let parsed = parse_file_actions_in_text("```file\n{\"action\":\"nope\"}\n```");
        assert_eq!(parsed.len(), 1);
        assert!(matches!(parsed[0], ParsedFileAction::Invalid(_)));
    }

    #[test]
    fn ignores_unclosed_and_other_fences() {
        assert!(parse_file_actions_in_text("```file\n{\"action\":\"read\"}").is_empty());
        assert!(parse_file_actions_in_text("```ui\n{}\n```").is_empty());
        assert!(
            parse_file_actions_in_text("```finter-file\n{\"action\":\"read\"}\n```").len() == 1
        );
    }

    #[test]
    fn rejects_binary_extensions_but_accepts_text() {
        for path in ["a.png", "a.pdf", "a.zip", "a.wasm", "a.exe"] {
            assert!(!is_text_file_path(path), "{path} should be binary");
        }
        for path in [
            "a.md",
            "src/main.rs",
            "notes",
            "a.finter.json",
            "Dockerfile",
        ] {
            assert!(is_text_file_path(path), "{path} should be text");
        }
        assert!(is_layout_path("a.finter.json"));
        assert!(is_layout_path("Macro/Crypto.FINTER.JSON"));
        assert!(!is_layout_path("a.json"));
    }

    #[test]
    fn action_accessors() {
        let write = FileAction::Write {
            path: Some("a.txt".into()),
            content: "x".into(),
            force: true,
        };
        assert_eq!(write.action_name(), "write");
        assert_eq!(write.path(), Some("a.txt"));
        assert_eq!(write.content(), Some("x"));
        assert!(write.force());
        let read = FileAction::Read { paths: vec![] };
        assert_eq!(read.path(), None);
        assert_eq!(read.content(), None);
        assert!(!read.force());
    }
}
