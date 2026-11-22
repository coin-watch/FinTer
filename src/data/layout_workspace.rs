//! Filesystem-backed layout workspace.
//!
//! Layouts are ordinary files inside a folder the user chooses. The folder is
//! the source of truth: subfolders are shown as folders in the explorer, and
//! each `<name>.finter.json` file is one layout. Nothing here contacts the
//! network.
//!
//! The workspace root is remembered in `~/.config/finter/workspace.json`. When
//! no root has been chosen yet we fall back to the old dashboards directory so
//! existing installs keep their layouts.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use tracing::{debug, info, warn};

use crate::data::error::{CoinWatchError, Result};
use crate::data::models::{LayoutEntry, LayoutEntryKind, Module};

/// New layout files use this suffix. The display name is the part before it.
pub const LAYOUT_EXTENSION: &str = ".finter.json";

/// The state that must survive restarts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceState {
    #[serde(default)]
    pub root: Option<String>,
    #[serde(default)]
    pub expanded_folders: Vec<String>,
    #[serde(default)]
    pub active_layout: Option<String>,
}

/// A loaded workspace tree plus the layout documents it contains.
#[derive(Debug, Clone)]
pub struct WorkspaceSnapshot {
    pub root: PathBuf,
    pub root_name: String,
    pub entries: Vec<LayoutEntry>,
    pub documents: Vec<Module>,
}

/// A layout loaded from a file.
#[derive(Debug, Clone)]
pub struct LayoutDocument {
    pub path: String,
    pub name: String,
    pub tree: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl LayoutDocument {
    pub fn to_module(&self) -> Module {
        Module {
            id: Some(self.path.clone()),
            user_id: None,
            module_name: self.name.clone(),
            module: self.tree.clone(),
            created_at: Some(self.created_at.to_rfc3339()),
            updated_at: Some(self.updated_at.to_rfc3339()),
        }
    }
}

// ──────────────────────────────────────────────
// Workspace configuration
// ──────────────────────────────────────────────

pub fn config_dir() -> Result<PathBuf> {
    dirs::config_dir()
        .map(|p| p.join("finter"))
        .ok_or_else(|| CoinWatchError::Storage("cannot determine config directory".into()))
}

pub fn state_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("workspace.json"))
}

/// The app-managed fallback workspace.
pub fn default_root() -> Result<PathBuf> {
    Ok(config_dir()?.join("dashboards"))
}

pub fn load_state() -> WorkspaceState {
    let Ok(path) = state_path() else {
        return WorkspaceState::default();
    };
    let Ok(json) = std::fs::read_to_string(&path) else {
        return WorkspaceState::default();
    };
    serde_json::from_str(&json).unwrap_or_else(|error| {
        warn!(?error, path = %path.display(), "ignoring invalid workspace state");
        WorkspaceState::default()
    })
}

pub fn save_state(state: &WorkspaceState) -> Result<()> {
    let path = state_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoinWatchError::Storage(format!("mkdir {}: {e}", parent.display())))?;
    }
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| CoinWatchError::Deserialization(format!("serialize workspace: {e}")))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)
        .map_err(|e| CoinWatchError::Storage(format!("write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| CoinWatchError::Storage(format!("rename {}: {e}", path.display())))?;
    Ok(())
}

/// The current workspace root, creating the fallback directory if necessary.
pub fn root() -> Result<PathBuf> {
    let state = load_state();
    let root = state
        .root
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or(default_root()?);
    std::fs::create_dir_all(&root)
        .map_err(|e| CoinWatchError::Storage(format!("create {}: {e}", root.display())))?;
    Ok(root)
}

/// Switch to a user-chosen workspace. The path does not need to exist yet; it
/// is created so the explorer always has somewhere to write.
pub fn set_root(path: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(path)
        .map_err(|e| CoinWatchError::Storage(format!("create {}: {e}", path.display())))?;
    let root = std::fs::canonicalize(path)
        .map_err(|e| CoinWatchError::Storage(format!("canonicalize {}: {e}", path.display())))?;
    let mut state = load_state();
    state.root = Some(root.to_string_lossy().to_string());
    state.active_layout = None;
    state.expanded_folders.clear();
    save_state(&state)?;
    info!(root = %root.display(), "layout workspace changed");
    Ok(root)
}

pub fn active_layout() -> Option<String> {
    load_state().active_layout
}

pub fn set_active_layout(path: Option<&str>) -> Result<()> {
    let mut state = load_state();
    state.active_layout = path.map(ToString::to_string);
    save_state(&state)
}

pub fn expanded_folders() -> Vec<String> {
    load_state().expanded_folders
}

pub fn set_expanded_folders(folders: &[String]) -> Result<()> {
    let mut state = load_state();
    state.expanded_folders = folders.to_vec();
    save_state(&state)
}

// ──────────────────────────────────────────────
// Path helpers
// ──────────────────────────────────────────────

fn storage_path(rel: &str, what: &str) -> CoinWatchError {
    CoinWatchError::Storage(format!("invalid {what} path: {rel:?}"))
}

/// Validate a relative path from the explorer. Empty means the workspace root.
fn validate_rel(rel: &str) -> Result<()> {
    if rel.is_empty() {
        return Ok(());
    }
    let path = Path::new(rel);
    if path.is_absolute() {
        return Err(storage_path(rel, "workspace"));
    }
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            _ => return Err(storage_path(rel, "workspace")),
        }
    }
    Ok(())
}

fn join_rel(root: &Path, rel: &str) -> Result<PathBuf> {
    validate_rel(rel)?;
    let mut path = root.to_path_buf();
    for part in rel.split('/') {
        if !part.is_empty() {
            path.push(part);
        }
    }
    Ok(path)
}

fn rel_string(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn display_name(rel: &str) -> String {
    let name = Path::new(rel)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| rel.to_string());
    if let Some(name) = name.strip_suffix(LAYOUT_EXTENSION) {
        return name.to_string();
    }
    if let Some(name) = name.strip_suffix(".json") {
        return name.to_string();
    }
    name
}

fn has_legacy_uuid_stem(rel: &str) -> bool {
    let stem = Path::new(rel)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let stem = stem
        .strip_suffix(LAYOUT_EXTENSION)
        .or_else(|| stem.strip_suffix(".json"))
        .unwrap_or(&stem);
    uuid::Uuid::parse_str(stem).is_ok()
}

fn file_extension(rel: &str) -> String {
    if rel.ends_with(LAYOUT_EXTENSION) {
        return LAYOUT_EXTENSION.to_string();
    }
    Path::new(rel)
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy()))
        .unwrap_or_default()
}

/// Name shown for a regular file: everything before its final extension.
fn file_stem_name(name: &str) -> String {
    if let Some(stem) = name.strip_suffix(LAYOUT_EXTENSION) {
        return stem.to_string();
    }
    Path::new(name)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| name.to_string())
}

fn sanitize_name(raw: &str) -> String {
    let mut name = raw.trim().to_string();
    if !name.starts_with('.') {
        name = name.trim_matches('.').to_string();
    }
    let name: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect();
    let mut name = name.trim().to_string();
    if !name.starts_with('.') {
        name = name.trim_matches('.').trim().to_string();
    }
    if name == "." || name == ".." {
        name.clear();
    }
    if name.is_empty() {
        "Untitled".to_string()
    } else {
        name
    }
}

fn layout_base_name(raw: &str) -> String {
    let name = sanitize_name(raw);
    name.strip_suffix(LAYOUT_EXTENSION)
        .or_else(|| name.strip_suffix(".json"))
        .unwrap_or(&name)
        .to_string()
}

/// Keep writes inside the workspace even if a caller hands us a bad path.
fn ensure_inside(root: &Path, path: &Path) -> Result<()> {
    let canonical_root = std::fs::canonicalize(root).map_err(|e| {
        CoinWatchError::Storage(format!("canonicalize workspace {}: {e}", root.display()))
    })?;
    let canonical_path = std::fs::canonicalize(path)
        .map_err(|e| CoinWatchError::Storage(format!("canonicalize {}: {e}", path.display())))?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err(CoinWatchError::InvalidRequest(format!(
            "path escapes the layout workspace: {}",
            path.display()
        )));
    }
    Ok(())
}

// ──────────────────────────────────────────────
// Layout files
// ──────────────────────────────────────────────

fn parse_timestamp(value: Option<&Value>) -> Option<DateTime<Utc>> {
    let value = value?;
    if let Some(text) = value.as_str() {
        return DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|dt| dt.with_timezone(&Utc));
    }
    value
        .as_i64()
        .and_then(|seconds| DateTime::from_timestamp(seconds, 0))
}

type ParsedLayout = (
    Value,
    Option<String>,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
);

/// Accept both the current `{tree: ...}` envelope and legacy bare trees.
fn parse_layout_value(value: Value) -> Option<ParsedLayout> {
    let tree = match value.get("tree") {
        Some(tree) if !tree.is_null() => tree.clone(),
        _ if value.get("kind").is_some() || value.get("Pane").is_some() => value.clone(),
        _ => return None,
    };

    let name = value
        .get("name")
        .or_else(|| value.get("module_name"))
        .and_then(|v| v.as_str())
        .map(ToString::to_string);
    let created = parse_timestamp(value.get("created_at"));
    let updated = parse_timestamp(value.get("updated_at"));
    Some((tree, name, created, updated))
}

fn read_layout_at(root: &Path, rel: &str) -> Result<LayoutDocument> {
    let path = join_rel(root, rel)?;
    ensure_inside(root, &path)?;
    let json = std::fs::read_to_string(&path)
        .map_err(|e| CoinWatchError::Storage(format!("read {}: {e}", path.display())))?;
    let value: Value = serde_json::from_str(&json)
        .map_err(|e| CoinWatchError::Deserialization(format!("parse {}: {e}", path.display())))?;
    let (tree, stored_name, created, updated) = parse_layout_value(value).ok_or_else(|| {
        CoinWatchError::Deserialization(format!("{} is not a layout file", path.display()))
    })?;

    let metadata = std::fs::metadata(&path).ok();
    let modified = metadata
        .as_ref()
        .and_then(|m| m.modified().ok())
        .map(DateTime::<Utc>::from);
    // The filename is the layout name. Keep a stored name only for legacy
    // UUID-named files, where the stem is meaningless to a person.
    let name = if has_legacy_uuid_stem(rel) {
        stored_name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| display_name(rel))
    } else {
        display_name(rel)
    };

    Ok(LayoutDocument {
        path: rel.to_string(),
        name,
        tree,
        created_at: created.or(modified).unwrap_or_else(Utc::now),
        updated_at: updated.or(modified).unwrap_or_else(Utc::now),
    })
}

fn write_layout_at(root: &Path, rel: &str, tree: Value, created: DateTime<Utc>) -> Result<()> {
    let path = join_rel(root, rel)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoinWatchError::Storage(format!("mkdir {}: {e}", parent.display())))?;
        ensure_inside(root, parent)?;
    }
    let now = Utc::now();
    let envelope = serde_json::json!({
        "version": 1,
        "name": display_name(rel),
        "tree": tree,
        "created_at": created.to_rfc3339(),
        "updated_at": now.to_rfc3339(),
    });
    let json = serde_json::to_string_pretty(&envelope)
        .map_err(|e| CoinWatchError::Deserialization(format!("serialize layout: {e}")))?;
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|s| s.to_str()).unwrap_or("json")
    ));
    std::fs::write(&tmp, json)
        .map_err(|e| CoinWatchError::Storage(format!("write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| CoinWatchError::Storage(format!("rename {}: {e}", path.display())))?;
    Ok(())
}

fn is_layout_candidate(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    name.ends_with(LAYOUT_EXTENSION) || name.ends_with(".json")
}

// ──────────────────────────────────────────────
// Directory scanning
// ──────────────────────────────────────────────

fn scan_dir(root: &Path, dir: &Path, rel_prefix: &str) -> Vec<LayoutEntry> {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        warn!(dir = %dir.display(), "cannot read layout folder");
        return Vec::new();
    };

    let mut folders = Vec::new();
    let mut files = Vec::new();

    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() && name.starts_with('.') {
            // Keep editor/VCS metadata folders out of the tree.
            continue;
        }
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        let rel = if rel_prefix.is_empty() {
            name.clone()
        } else {
            format!("{rel_prefix}/{name}")
        };

        if file_type.is_dir() {
            let children = scan_dir(root, &path, &rel);
            let layout_count = children.iter().map(|child| child.layout_count).sum();
            folders.push(LayoutEntry {
                path: rel,
                name,
                kind: LayoutEntryKind::Folder,
                children,
                layout_count,
                updated_at: entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .map(DateTime::<Utc>::from),
            });
        } else if file_type.is_file() {
            let updated_at = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .map(DateTime::<Utc>::from);
            if is_layout_candidate(&path) {
                if let Ok(document) = read_layout_at(root, &rel) {
                    files.push(LayoutEntry {
                        path: rel,
                        name: document.name,
                        kind: LayoutEntryKind::Layout,
                        children: Vec::new(),
                        layout_count: 1,
                        updated_at,
                    });
                    continue;
                }
                debug!(path = %path.display(), "showing non-layout json file as a regular file");
            }
            files.push(LayoutEntry {
                path: rel,
                name: file_stem_name(&name),
                kind: LayoutEntryKind::File,
                children: Vec::new(),
                layout_count: 0,
                updated_at,
            });
        }
    }

    folders.sort_by_cached_key(|entry| entry.name.to_lowercase());
    files.sort_by_cached_key(|entry| entry.name.to_lowercase());
    folders.extend(files);
    folders
}

fn collect_documents(root: &Path, entries: &[LayoutEntry], out: &mut Vec<Module>) {
    for entry in entries {
        match entry.kind {
            LayoutEntryKind::Folder => collect_documents(root, &entry.children, out),
            LayoutEntryKind::Layout => {
                if let Ok(document) = read_layout_at(root, &entry.path) {
                    out.push(document.to_module());
                }
            }
            LayoutEntryKind::File => {}
        }
    }
}

/// Read the whole workspace.
pub fn snapshot() -> Result<WorkspaceSnapshot> {
    let root = root()?;
    let root_name = root
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| root.to_string_lossy().to_string());
    let entries = scan_dir(&root, &root, "");
    let mut documents = Vec::new();
    collect_documents(&root, &entries, &mut documents);
    Ok(WorkspaceSnapshot {
        root,
        root_name,
        entries,
        documents,
    })
}

/// Load one layout document by relative path.
pub fn load_document(rel: &str) -> Result<LayoutDocument> {
    let root = root()?;
    read_layout_at(&root, rel)
}

/// Whether a file currently parses as a FinTer layout.
pub fn is_layout_file(rel: &str) -> bool {
    let Ok(root) = root() else {
        return false;
    };
    read_layout_at(&root, rel).is_ok()
}

/// Flatten the tree into layout entries only.
pub fn flatten_layouts(entries: &[LayoutEntry]) -> Vec<&LayoutEntry> {
    let mut layouts = Vec::new();
    fn walk<'a>(entries: &'a [LayoutEntry], out: &mut Vec<&'a LayoutEntry>) {
        for entry in entries {
            match entry.kind {
                LayoutEntryKind::Folder => walk(&entry.children, out),
                LayoutEntryKind::Layout => out.push(entry),
                LayoutEntryKind::File => {}
            }
        }
    }
    walk(entries, &mut layouts);
    layouts
}

/// List all folder paths, including the workspace root as `""`.
pub fn folder_paths(entries: &[LayoutEntry]) -> Vec<(String, String, usize)> {
    fn walk(entries: &[LayoutEntry], out: &mut Vec<(String, String, usize)>, depth: usize) {
        for entry in entries {
            if matches!(entry.kind, LayoutEntryKind::Folder) {
                out.push((entry.path.clone(), entry.name.clone(), depth));
                walk(&entry.children, out, depth + 1);
            }
        }
    }
    let mut folders = vec![(String::new(), "Workspace root".to_string(), 0)];
    walk(entries, &mut folders, 1);
    folders
}

// ──────────────────────────────────────────────
// Mutations
// ──────────────────────────────────────────────

fn unique_named_path(parent: &Path, file_name: &str) -> PathBuf {
    let candidate = parent.join(file_name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(file_name);
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| file_name.to_string());
    let extension = path
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy()))
        .unwrap_or_default();
    let mut counter = 2;
    loop {
        let candidate = parent.join(format!("{stem} {counter}{extension}"));
        if !candidate.exists() {
            return candidate;
        }
        counter += 1;
    }
}

fn unique_path(parent: &Path, base: &str, extension: &str) -> PathBuf {
    let mut candidate = parent.join(format!("{base}{extension}"));
    let mut counter = 2;
    while candidate.exists() {
        candidate = parent.join(format!("{base} {counter}{extension}"));
        counter += 1;
    }
    candidate
}

fn empty_layout_tree() -> Value {
    serde_json::json!({
        "kind": "pane",
        "id": uuid::Uuid::new_v4().to_string(),
        "data": { "type": "Empty", "config": {} }
    })
}

/// Create a new layout in `parent_rel` (empty means root).
pub fn create_layout(parent_rel: &str, name: &str) -> Result<LayoutDocument> {
    let root = root()?;
    let parent = join_rel(&root, parent_rel)?;
    std::fs::create_dir_all(&parent)
        .map_err(|e| CoinWatchError::Storage(format!("mkdir {}: {e}", parent.display())))?;
    ensure_inside(&root, &parent)?;
    let base = layout_base_name(name);
    let path = unique_path(&parent, &base, LAYOUT_EXTENSION);
    let rel = rel_string(path.strip_prefix(&root).unwrap_or(&path));
    write_layout_at(&root, &rel, empty_layout_tree(), Utc::now())?;
    info!(path = %rel, "created layout");
    read_layout_at(&root, &rel)
}

/// Create a regular file with exactly the name supplied (extension included).
pub fn create_file(parent_rel: &str, name: &str) -> Result<String> {
    let root = root()?;
    let parent = join_rel(&root, parent_rel)?;
    std::fs::create_dir_all(&parent)
        .map_err(|e| CoinWatchError::Storage(format!("mkdir {}: {e}", parent.display())))?;
    ensure_inside(&root, &parent)?;
    let file_name = sanitize_name(name);
    let path = unique_named_path(&parent, &file_name);
    std::fs::write(&path, b"")
        .map_err(|e| CoinWatchError::Storage(format!("write {}: {e}", path.display())))?;
    Ok(rel_string(path.strip_prefix(&root).unwrap_or(&path)))
}

/// Create a folder inside `parent_rel`.
pub fn create_folder(parent_rel: &str, name: &str) -> Result<String> {
    let root = root()?;
    let parent = join_rel(&root, parent_rel)?;
    std::fs::create_dir_all(&parent)
        .map_err(|e| CoinWatchError::Storage(format!("mkdir {}: {e}", parent.display())))?;
    ensure_inside(&root, &parent)?;
    let base = sanitize_name(name);
    let mut path = parent.join(&base);
    let mut counter = 2;
    while path.exists() {
        path = parent.join(format!("{base} {counter}"));
        counter += 1;
    }
    std::fs::create_dir(&path)
        .map_err(|e| CoinWatchError::Storage(format!("mkdir {}: {e}", path.display())))?;
    Ok(rel_string(path.strip_prefix(&root).unwrap_or(&path)))
}

/// Save a layout tree back to its file.
pub fn save_layout(rel: &str, tree: Value) -> Result<()> {
    let root = root()?;
    let path = join_rel(&root, rel)?;
    ensure_inside(&root, &path)?;
    let created = read_layout_at(&root, rel)
        .map(|doc| doc.created_at)
        .unwrap_or_else(|_| Utc::now());
    write_layout_at(&root, rel, tree, created)
}

/// Absolute path for an entry inside the current workspace.
pub fn absolute_path(rel: &str) -> Result<PathBuf> {
    let root = root()?;
    let path = join_rel(&root, rel)?;
    if path.exists() {
        ensure_inside(&root, &path)?;
    }
    Ok(path)
}

/// Read any file in the workspace as bytes.
pub fn read_bytes(rel: &str) -> Result<Vec<u8>> {
    let root = root()?;
    let path = join_rel(&root, rel)?;
    ensure_inside(&root, &path)?;
    std::fs::read(&path)
        .map_err(|e| CoinWatchError::Storage(format!("read {}: {e}", path.display())))
}

/// Save UTF-8 text back to any file in the workspace.
pub fn write_text(rel: &str, content: &str) -> Result<()> {
    let root = root()?;
    write_text_at(&root, rel, content, false)
}

/// Save UTF-8 text, creating missing parent folders for files the Agent creates.
pub fn write_text_creating(rel: &str, content: &str) -> Result<()> {
    let root = root()?;
    write_text_at(&root, rel, content, true)
}

fn write_text_at(root: &Path, rel: &str, content: &str, create_parents: bool) -> Result<()> {
    let path = join_rel(root, rel)?;
    if path.exists() {
        ensure_inside(root, &path)?;
        if path.is_dir() {
            return Err(CoinWatchError::InvalidRequest(format!(
                "\"{rel}\" is a folder"
            )));
        }
    }
    let parent = path.parent().unwrap_or(root);
    if create_parents {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoinWatchError::Storage(format!("mkdir {}: {e}", parent.display())))?;
    }
    ensure_inside(root, parent)?;
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|s| s.to_str()).unwrap_or("file")
    ));
    std::fs::write(&tmp, content)
        .map_err(|e| CoinWatchError::Storage(format!("write {}: {e}", tmp.display())))?;
    if let Ok(metadata) = std::fs::metadata(&path) {
        let _ = std::fs::set_permissions(&tmp, metadata.permissions());
    }
    std::fs::rename(&tmp, &path)
        .map_err(|e| CoinWatchError::Storage(format!("rename {}: {e}", path.display())))?;
    Ok(())
}

/// Append UTF-8 text to a workspace file, creating it (and missing parent
/// folders) when needed.
pub fn append_text(rel: &str, content: &str) -> Result<()> {
    let root = root()?;
    append_text_at(&root, rel, content)
}

fn append_text_at(root: &Path, rel: &str, content: &str) -> Result<()> {
    use std::io::Write;

    let path = join_rel(root, rel)?;
    if path.exists() {
        ensure_inside(root, &path)?;
        if path.is_dir() {
            return Err(CoinWatchError::InvalidRequest(format!(
                "\"{rel}\" is a folder"
            )));
        }
    } else {
        let parent = path.parent().unwrap_or(root);
        std::fs::create_dir_all(parent)
            .map_err(|e| CoinWatchError::Storage(format!("mkdir {}: {e}", parent.display())))?;
        ensure_inside(root, parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| CoinWatchError::Storage(format!("open {}: {e}", path.display())))?;
    file.write_all(content.as_bytes())
        .map_err(|e| CoinWatchError::Storage(format!("append {}: {e}", path.display())))?;
    Ok(())
}

/// Rename a layout or folder. Returns the new relative path.
pub fn rename(rel: &str, new_name: &str) -> Result<String> {
    let root = root()?;
    let path = join_rel(&root, rel)?;
    ensure_inside(&root, &path)?;
    if !path.exists() {
        return Err(CoinWatchError::NotFound(rel.to_string()));
    }
    let parent = path.parent().unwrap_or(&root);
    let is_dir = path.is_dir();
    let extension = if is_dir {
        String::new()
    } else {
        file_extension(rel)
    };
    let mut base = sanitize_name(new_name);
    if !extension.is_empty() && base.ends_with(&extension) {
        base.truncate(base.len() - extension.len());
        base = base.trim_end_matches('.').trim().to_string();
    }
    if base.is_empty() {
        return Err(CoinWatchError::InvalidRequest(
            "a file name is required".into(),
        ));
    }
    let target = parent.join(format!("{base}{extension}"));
    if target == path {
        return Ok(rel.to_string());
    }
    if target.exists() {
        return Err(CoinWatchError::InvalidRequest(format!(
            "\"{}\" already exists in this folder",
            target.file_name().and_then(|s| s.to_str()).unwrap_or(&base)
        )));
    }
    std::fs::rename(&path, &target)
        .map_err(|e| CoinWatchError::Storage(format!("rename {}: {e}", path.display())))?;
    Ok(rel_string(target.strip_prefix(&root).unwrap_or(&target)))
}

/// Duplicate any file next to the original.
pub fn duplicate(rel: &str) -> Result<String> {
    let root = root()?;
    let source = join_rel(&root, rel)?;
    ensure_inside(&root, &source)?;
    if !source.is_file() {
        return Err(CoinWatchError::InvalidRequest(
            "only files can be duplicated".into(),
        ));
    }
    let parent = source.parent().unwrap_or(&root);
    let extension = file_extension(rel);
    let source_name = source
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| rel.to_string());

    let target = if let Ok(document) = read_layout_at(&root, rel) {
        let base = sanitize_name(&format!("{} copy", document.name));
        let target = unique_path(parent, &base, &extension);
        let target_rel = rel_string(target.strip_prefix(&root).unwrap_or(&target));
        write_layout_at(&root, &target_rel, document.tree, Utc::now())?;
        target
    } else {
        let base = sanitize_name(&format!("{} copy", file_stem_name(&source_name)));
        let target = unique_path(parent, &base, &extension);
        let bytes = std::fs::read(&source)
            .map_err(|e| CoinWatchError::Storage(format!("read {}: {e}", source.display())))?;
        std::fs::write(&target, bytes)
            .map_err(|e| CoinWatchError::Storage(format!("write {}: {e}", target.display())))?;
        target
    };

    Ok(rel_string(target.strip_prefix(&root).unwrap_or(&target)))
}

/// Move a file or folder into `dest_dir_rel` (empty means root).
pub fn move_entry(rel: &str, dest_dir_rel: &str) -> Result<String> {
    let root = root()?;
    let source = join_rel(&root, rel)?;
    let dest_dir = join_rel(&root, dest_dir_rel)?;
    ensure_inside(&root, &source)?;
    if !source.exists() {
        return Err(CoinWatchError::NotFound(rel.to_string()));
    }
    std::fs::create_dir_all(&dest_dir)
        .map_err(|e| CoinWatchError::Storage(format!("mkdir {}: {e}", dest_dir.display())))?;
    ensure_inside(&root, &dest_dir)?;
    if source.is_dir() {
        let source_canonical = std::fs::canonicalize(&source).map_err(|e| {
            CoinWatchError::Storage(format!("canonicalize {}: {e}", source.display()))
        })?;
        let dest_canonical = std::fs::canonicalize(&dest_dir).map_err(|e| {
            CoinWatchError::Storage(format!("canonicalize {}: {e}", dest_dir.display()))
        })?;
        if dest_canonical.starts_with(&source_canonical) {
            return Err(CoinWatchError::InvalidRequest(
                "cannot move a folder into itself".into(),
            ));
        }
    }
    let Some(file_name) = source.file_name() else {
        return Err(CoinWatchError::InvalidRequest("invalid source path".into()));
    };
    let target = dest_dir.join(file_name);
    if target.exists() {
        return Err(CoinWatchError::InvalidRequest(format!(
            "\"{}\" already exists in the destination folder",
            file_name.to_string_lossy()
        )));
    }
    std::fs::rename(&source, &target)
        .map_err(|e| CoinWatchError::Storage(format!("move {}: {e}", source.display())))?;
    Ok(rel_string(target.strip_prefix(&root).unwrap_or(&target)))
}

/// Delete a layout file or an entire folder.
pub fn delete(rel: &str) -> Result<()> {
    let root = root()?;
    let path = join_rel(&root, rel)?;
    if path == root {
        return Err(CoinWatchError::InvalidRequest(
            "cannot delete the workspace root".into(),
        ));
    }
    ensure_inside(&root, &path)?;
    if !path.exists() {
        return Ok(());
    }
    if path.is_dir() {
        std::fs::remove_dir_all(&path)
            .map_err(|e| CoinWatchError::Storage(format!("delete {}: {e}", path.display())))?;
    } else {
        std::fs::remove_file(&path)
            .map_err(|e| CoinWatchError::Storage(format!("delete {}: {e}", path.display())))?;
    }
    info!(path = %rel, "deleted layout entry");
    Ok(())
}

/// Count layouts below a folder, used by the delete confirmation.
pub fn layout_count(rel: &str) -> usize {
    let Ok(root) = root() else {
        return 0;
    };
    let Ok(path) = join_rel(&root, rel) else {
        return 0;
    };
    fn count(dir: &Path, root: &Path) -> usize {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        entries
            .flatten()
            .map(|entry| {
                let path = entry.path();
                let hidden_dir =
                    entry.file_name().to_string_lossy().starts_with('.') && path.is_dir();
                if hidden_dir {
                    0
                } else if path.is_dir() {
                    count(&path, root)
                } else if path.is_file() && is_layout_candidate(&path) {
                    let rel = rel_string(path.strip_prefix(root).unwrap_or(&path));
                    usize::from(read_layout_at(root, &rel).is_ok())
                } else {
                    0
                }
            })
            .sum()
    }
    count(&path, &root)
}

/// Open the system file manager at a layout or folder.
pub fn reveal(rel: &str) -> Result<()> {
    let root = root()?;
    let path = join_rel(&root, rel)?;
    let target = if path.is_dir() {
        path.clone()
    } else {
        path.parent().unwrap_or(&root).to_path_buf()
    };

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&target)
            .spawn()
            .map_err(|e| CoinWatchError::Storage(format!("open file manager: {e}")))?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-R")
            .arg(&path)
            .spawn()
            .map_err(|e| CoinWatchError::Storage(format!("open file manager: {e}")))?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn()
            .map_err(|e| CoinWatchError::Storage(format!("open file manager: {e}")))?;
    }
    Ok(())
}

/// Resolve a legacy UUID tab id to the relative path of its file, if present.
pub fn resolve_legacy_id(entries: &[LayoutEntry], legacy_id: &str) -> Option<String> {
    for entry in flatten_layouts(entries) {
        let stem = Path::new(&entry.path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if stem == format!("{legacy_id}.json")
            || stem == format!("{legacy_id}{LAYOUT_EXTENSION}")
            || entry.path == legacy_id
        {
            return Some(entry.path.clone());
        }
    }
    None
}

/// Remove duplicate paths while preserving order.
pub fn dedup(paths: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("finter-workspace-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rejects_traversal() {
        assert!(validate_rel("../escape").is_err());
        assert!(validate_rel("/absolute").is_err());
        assert!(validate_rel("a/../b").is_err());
        assert!(validate_rel("a/b").is_ok());
        assert!(validate_rel("").is_ok());
    }

    #[test]
    fn sanitizes_names() {
        assert_eq!(sanitize_name("My/Layout"), "My-Layout");
        assert_eq!(sanitize_name("  "), "Untitled");
        assert_eq!(sanitize_name("x.finter.json"), "x.finter.json");
    }

    #[test]
    fn agent_writes_create_nested_files_and_append() {
        let root = scratch("agent-write");
        write_text_at(&root, "notes/deep/plan.md", "# Plan", true).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("notes/deep/plan.md")).unwrap(),
            "# Plan"
        );
        append_text_at(&root, "notes/deep/plan.md", "\n- step").unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("notes/deep/plan.md")).unwrap(),
            "# Plan\n- step"
        );
        append_text_at(&root, "fresh/log.txt", "new").unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("fresh/log.txt")).unwrap(),
            "new"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn agent_writes_reject_traversal_and_folders() {
        let root = scratch("agent-write-guard");
        assert!(write_text_at(&root, "../escape.txt", "x", true).is_err());
        assert!(append_text_at(&root, "/absolute.txt", "x").is_err());
        std::fs::create_dir_all(root.join("folder")).unwrap();
        assert!(write_text_at(&root, "folder", "x", false).is_err());
        assert!(append_text_at(&root, "folder", "x").is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn scan_reads_nested_layouts() {
        let root = scratch("scan");
        std::fs::create_dir_all(root.join("Swing")).unwrap();
        write_layout_at(&root, "root.finter.json", empty_layout_tree(), Utc::now()).unwrap();
        write_layout_at(
            &root,
            "Swing/child.finter.json",
            empty_layout_tree(),
            Utc::now(),
        )
        .unwrap();

        let entries = scan_dir(&root, &root, "");
        assert_eq!(entries.len(), 2);
        assert!(matches!(entries[0].kind, LayoutEntryKind::Folder));
        assert_eq!(entries[0].layout_count, 1);
        assert_eq!(entries[1].path, "root.finter.json");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn scan_includes_regular_files() {
        let root = scratch("regular-files");
        std::fs::write(root.join("notes.md"), "# notes").unwrap();
        std::fs::write(root.join("config.yaml"), "key: value").unwrap();
        write_layout_at(&root, "layout.finter.json", empty_layout_tree(), Utc::now()).unwrap();

        let entries = scan_dir(&root, &root, "");
        assert_eq!(entries.len(), 3);
        let notes = entries.iter().find(|e| e.path == "notes.md").unwrap();
        assert!(matches!(notes.kind, LayoutEntryKind::File));
        assert_eq!(notes.name, "notes");
        assert_eq!(file_extension("notes.md"), ".md");
        assert_eq!(file_stem_name("notes.md"), "notes");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn legacy_bare_tree_is_readable() {
        let root = scratch("legacy");
        std::fs::write(
            root.join("old.json"),
            r#"{"kind":"pane","id":"p1","data":{"type":"Empty","config":{}}}"#,
        )
        .unwrap();

        let doc = read_layout_at(&root, "old.json").unwrap();
        assert_eq!(doc.path, "old.json");
        assert_eq!(doc.tree["kind"], "pane");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn unique_file_names_keep_their_extension() {
        let root = scratch("unique-file");
        std::fs::write(root.join("notes.md"), "").unwrap();
        assert_eq!(
            unique_named_path(&root, "notes.md")
                .file_name()
                .unwrap()
                .to_string_lossy(),
            "notes 2.md"
        );
        assert_eq!(
            unique_named_path(&root, "README")
                .file_name()
                .unwrap()
                .to_string_lossy(),
            "README"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn unique_names_do_not_overwrite() {
        let root = scratch("unique");
        let parent = root.join("folder");
        std::fs::create_dir_all(&parent).unwrap();
        assert_eq!(
            unique_path(&parent, "Layout", LAYOUT_EXTENSION)
                .file_name()
                .unwrap(),
            "Layout.finter.json"
        );
        std::fs::write(parent.join("Layout.finter.json"), "{}").unwrap();
        assert_eq!(
            unique_path(&parent, "Layout", LAYOUT_EXTENSION)
                .file_name()
                .unwrap(),
            "Layout 2.finter.json"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
