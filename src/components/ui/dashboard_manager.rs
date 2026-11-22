//! Local filesystem explorer for FinTer layouts.
//!
//! The sidebar is intentionally quiet: rows contain an icon, a name and an
//! open-tab marker. All file operations live in a full right-click context
//! menu, the same way an editor project panel behaves. There are no per-row
//! delete or action buttons.

use crate::data::layout_workspace::{self, LAYOUT_EXTENSION};
use crate::data::models::{LayoutEntry, LayoutEntryKind};
use gpui::{
    App, ClipboardItem, Context, ElementId, EventEmitter, FocusHandle, Focusable, FontWeight, Hsla,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, Render,
    SharedString, Stateful, StatefulInteractiveElement, Styled, WeakEntity, Window, div,
    prelude::*, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_component::{ActiveTheme, Icon, Sizable};
use std::collections::HashSet;
use std::path::Path;

/// Events the explorer raises to `MainView`, which owns the filesystem and the
/// editor tabs.
pub enum DashboardManagerEvent {
    OpenLayout(String),
    OpenFile(String),
    NewFile(String),
    DetachLayout(String),
    CloseLayout(String),
    NewLayout(String),
    NewFolder(String),
    RenameEntry {
        path: String,
        name: String,
    },
    DuplicateEntry(String),
    MoveEntry {
        path: String,
        dest: String,
    },
    DeleteEntry {
        path: String,
        name: String,
        is_folder: bool,
        child_count: usize,
    },
    RevealEntry(String),
    OpenWorkspace,
    Refresh,
}

const ROW_HEIGHT: f32 = 24.0;
const INDENT: f32 = 12.0;

pub struct DashboardManager {
    focus_handle: FocusHandle,
    entries: Vec<LayoutEntry>,
    root_name: String,
    active_path: Option<String>,
    open_paths: Vec<String>,
    expanded: HashSet<String>,
    root_expanded: bool,
    selected_path: Option<String>,
    filter: String,
    filter_input: Option<gpui::Entity<InputState>>,
    rename_path: Option<String>,
    rename_input: Option<gpui::Entity<InputState>>,
    pub error: Option<String>,
}

impl DashboardManager {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            entries: Vec::new(),
            root_name: "Layouts".to_string(),
            active_path: layout_workspace::active_layout(),
            open_paths: Vec::new(),
            expanded: layout_workspace::expanded_folders().into_iter().collect(),
            root_expanded: true,
            selected_path: None,
            filter: String::new(),
            filter_input: None,
            rename_path: None,
            rename_input: None,
            error: None,
        }
    }

    pub fn update_tree(
        &mut self,
        entries: Vec<LayoutEntry>,
        root_name: String,
        cx: &mut Context<Self>,
    ) {
        self.entries = entries;
        self.root_name = root_name;
        self.expanded
            .retain(|path| find_entry(&self.entries, path).is_some());
        if self.expanded.is_empty() {
            // First run (or a new workspace): reveal one level so the tree
            // does not look empty.
            self.expanded = self
                .entries
                .iter()
                .filter(|entry| matches!(entry.kind, LayoutEntryKind::Folder))
                .map(|entry| entry.path.clone())
                .collect();
        }
        if self
            .selected_path
            .as_ref()
            .is_some_and(|path| self.entry_exists(path).is_none())
        {
            self.selected_path = None;
        }
        if self
            .active_path
            .as_ref()
            .is_some_and(|path| self.entry_exists(path).is_none())
        {
            self.active_path = None;
        }
        cx.notify();
    }

    pub fn update_error(&mut self, err: String, cx: &mut Context<Self>) {
        self.error = Some(err);
        cx.notify();
    }

    pub fn clear_error(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        cx.notify();
    }

    pub fn set_active_module(&mut self, path: String, cx: &mut Context<Self>) {
        self.active_path = Some(path.clone());
        self.selected_path = Some(path.clone());
        self.ensure_ancestors_expanded(&path);
        cx.notify();
    }

    pub fn clear_active_module(&mut self, cx: &mut Context<Self>) {
        self.active_path = None;
        cx.notify();
    }

    pub fn set_open_modules(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        self.open_paths = paths;
        cx.notify();
    }

    pub fn set_selected_path(&mut self, path: String, cx: &mut Context<Self>) {
        self.ensure_ancestors_expanded(&path);
        self.selected_path = Some(path);
        cx.notify();
    }

    /// Update explorer state after a file or folder is renamed or moved.
    pub fn remap_paths(&mut self, old: &str, new: &str, cx: &mut Context<Self>) {
        let remap = |path: &str| -> Option<String> {
            if path == old {
                return Some(new.to_string());
            }
            path.strip_prefix(old)
                .and_then(|suffix| suffix.strip_prefix('/'))
                .map(|suffix| format!("{new}/{suffix}"))
        };

        self.expanded = self
            .expanded
            .iter()
            .filter_map(|path| remap(path))
            .collect();
        self.open_paths = self
            .open_paths
            .iter()
            .filter_map(|path| remap(path))
            .collect();
        if let Some(path) = self.selected_path.clone() {
            self.selected_path = remap(&path);
        }
        if let Some(path) = self.active_path.clone() {
            self.active_path = remap(&path);
        }
        self.persist_expanded();
        cx.notify();
    }

    pub fn begin_rename_path(&mut self, path: String, cx: &mut Context<Self>) {
        self.expanded.extend(ancestors_of(&path));
        self.selected_path = Some(path.clone());
        self.rename_path = Some(path);
        self.rename_input = None;
        cx.notify();
    }

    /// Close an inline rename editor without changing the path. Used when a
    /// name was already supplied (for example by an Agent directive).
    pub fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        self.rename_path = None;
        self.rename_input = None;
        cx.notify();
    }

    fn begin_rename_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.selected_path.clone() {
            self.begin_rename_path(path, cx);
        }
    }

    fn entry_exists(&self, path: &str) -> Option<&LayoutEntry> {
        find_entry(&self.entries, path)
    }

    fn entry_name(&self, path: &str) -> String {
        self.entry_exists(path)
            .map(|entry| entry.name.clone())
            .unwrap_or_else(|| path.to_string())
    }

    fn entry_is_folder(&self, path: &str) -> bool {
        self.entry_exists(path)
            .is_some_and(|entry| matches!(entry.kind, LayoutEntryKind::Folder))
    }

    fn ensure_ancestors_expanded(&mut self, path: &str) {
        for ancestor in ancestors_of(path) {
            self.expanded.insert(ancestor);
        }
    }

    fn persist_expanded(&self) {
        let mut folders: Vec<String> = self.expanded.iter().cloned().collect();
        folders.sort();
        if let Err(error) = layout_workspace::set_expanded_folders(&folders) {
            tracing::warn!(?error, "failed to persist expanded layout folders");
        }
    }

    fn toggle_folder(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.expanded.remove(&path) {
            self.expanded.insert(path);
        }
        self.persist_expanded();
        cx.notify();
    }

    fn filtered_entries(&self) -> Vec<LayoutEntry> {
        filter_entries(&self.entries, &self.filter)
    }

    fn visible_rows(&self) -> Vec<(String, String, LayoutEntryKind, usize)> {
        let mut rows = Vec::new();
        if self.root_expanded {
            let filtering = !self.filter.is_empty();
            collect_rows(
                &self.filtered_entries(),
                0,
                &self.expanded,
                filtering,
                &mut rows,
            );
        }
        rows
    }

    fn target_parent(&self) -> String {
        let Some(path) = self.selected_path.clone() else {
            return String::new();
        };
        if self.entry_is_folder(&path) {
            path
        } else {
            parent_of(&path)
        }
    }

    fn new_file(&mut self, cx: &mut Context<Self>) {
        cx.emit(DashboardManagerEvent::NewFile(self.target_parent()));
    }

    fn select_delta(&mut self, delta: isize, cx: &mut Context<Self>) {
        let rows = self.visible_rows();
        if rows.is_empty() {
            return;
        }
        let current = self
            .selected_path
            .as_deref()
            .and_then(|selected| rows.iter().position(|(path, _, _, _)| path == selected))
            .unwrap_or(0);
        let next = if delta < 0 {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            (current + delta as usize).min(rows.len() - 1)
        };
        self.selected_path = Some(rows[next].0.clone());
        cx.notify();
    }

    fn activate_selected(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.selected_path.clone() else {
            return;
        };
        if self.entry_is_folder(&path) {
            self.toggle_folder(path, cx);
        } else {
            cx.emit(DashboardManagerEvent::OpenLayout(path));
        }
    }

    fn delete_selected(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.selected_path.clone() else {
            return;
        };
        let Some(entry) = self.entry_exists(&path).cloned() else {
            return;
        };
        cx.emit(DashboardManagerEvent::DeleteEntry {
            path,
            name: entry.name,
            is_folder: matches!(entry.kind, LayoutEntryKind::Folder),
            child_count: entry.layout_count,
        });
    }

    fn collapse_selected_or_parent(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.selected_path.clone() else {
            return;
        };
        if self.entry_is_folder(&path) && self.expanded.contains(&path) {
            self.expanded.remove(&path);
            self.persist_expanded();
            cx.notify();
        } else {
            let parent = parent_of(&path);
            if !parent.is_empty() {
                self.selected_path = Some(parent);
                cx.notify();
            }
        }
    }

    fn expand_or_open_selected(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.selected_path.clone() else {
            return;
        };
        if self.entry_is_folder(&path) {
            if !self.expanded.contains(&path) {
                self.expanded.insert(path);
                self.persist_expanded();
                cx.notify();
            } else if let Some(first) = self
                .entry_exists(&path)
                .and_then(|entry| entry.children.first().map(|child| child.path.clone()))
            {
                self.selected_path = Some(first);
                cx.notify();
            }
        } else {
            cx.emit(DashboardManagerEvent::OpenLayout(path));
        }
    }

    fn move_targets(&self, source: &str, source_is_folder: bool) -> Vec<(String, String, usize)> {
        let parent = parent_of(source);
        let mut targets = layout_workspace::folder_paths(&self.entries);
        targets.retain(|(path, _, _)| {
            if path == &parent {
                return false;
            }
            if source_is_folder && (path == source || path.starts_with(&format!("{source}/"))) {
                return false;
            }
            true
        });
        targets
    }

    fn ensure_filter_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.filter_input.is_some() {
            return;
        }

        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Filter layouts")
                .default_value(self.filter.clone())
        });

        cx.subscribe(&input, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let value = input.read(cx).value().trim().to_string();
                if this.filter != value {
                    this.filter = value;
                    cx.notify();
                }
            }
        })
        .detach();

        self.filter_input = Some(input);
    }

    fn ensure_rename_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.rename_path.clone() else {
            self.rename_input = None;
            return;
        };
        if self.rename_input.is_some() {
            return;
        }

        let initial = self.entry_name(&path);
        let input = cx.new(|cx| InputState::new(window, cx).default_value(initial));
        cx.subscribe(&input, |this, input, event: &InputEvent, cx| match event {
            InputEvent::PressEnter { .. } => {
                let name = input.read(cx).value().trim().to_string();
                if let Some(path) = this.rename_path.take() {
                    this.rename_input = None;
                    if !name.is_empty() {
                        cx.emit(DashboardManagerEvent::RenameEntry { path, name });
                    }
                }
                cx.notify();
            }
            _ => {}
        })
        .detach();

        // Focus after the context menu has closed and the input is mounted.
        // Blur is deliberately not treated as cancel: closing the popup can
        // blur the freshly-created input for a frame.
        let focus_input = input.clone();
        window.defer(cx, move |window, cx| {
            focus_input.read(cx).focus_handle(cx).focus(window);
        });
        self.rename_input = Some(input);
    }

    fn cancel_rename_unless(&mut self, path: &str) {
        if self
            .rename_path
            .as_deref()
            .is_some_and(|renaming| renaming != path)
        {
            self.rename_path = None;
            self.rename_input = None;
        }
    }

    fn render_entries(
        &self,
        entries: &[LayoutEntry],
        depth: usize,
        filtering: bool,
        colors: &gpui_component::theme::ThemeColor,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let hover_bg = colors.foreground.opacity(0.05);
        let selected_bg = colors.foreground.opacity(0.08);
        let active_bg = colors.foreground.opacity(0.13);
        let muted = colors.muted_foreground;
        let foreground = colors.foreground;
        let mut rows = Vec::new();

        for entry in entries {
            let path = entry.path.clone();
            let name = entry.name.clone();
            let is_folder = matches!(entry.kind, LayoutEntryKind::Folder);
            let is_layout = matches!(entry.kind, LayoutEntryKind::Layout);
            let is_active = self.active_path.as_ref() == Some(&path);
            let is_selected = self.selected_path.as_ref() == Some(&path);
            let is_open = self.open_paths.contains(&path);
            let expanded = filtering || self.expanded.contains(&path);
            let targets = self.move_targets(&path, is_folder);
            let panel = cx.entity().downgrade();
            let rename_input = (self.rename_path.as_ref() == Some(&path))
                .then(|| self.rename_input.clone())
                .flatten();

            let mut row = row_shell(
                &path,
                depth,
                is_active,
                is_selected,
                hover_bg,
                active_bg,
                selected_bg,
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener({
                    let path = path.clone();
                    move |this, _event, window, cx| {
                        window.focus(&this.focus_handle);
                        this.cancel_rename_unless(&path);
                        this.selected_path = Some(path.clone());
                        cx.notify();
                    }
                }),
            );

            if is_folder {
                row = row
                    .on_click(cx.listener({
                        let path = path.clone();
                        move |this, _event, window, cx| {
                            window.focus(&this.focus_handle);
                            this.cancel_rename_unless(&path);
                            this.selected_path = Some(path.clone());
                            this.toggle_folder(path.clone(), cx);
                        }
                    }))
                    .child(
                        Icon::default()
                            .path(if expanded {
                                "icons/chevron-down.svg"
                            } else {
                                "icons/chevron-right.svg"
                            })
                            .xsmall()
                            .text_color(muted.opacity(0.7)),
                    )
                    .child(
                        Icon::default()
                            .path(if expanded {
                                "icons/folder-open.svg"
                            } else {
                                "icons/folder-closed.svg"
                            })
                            .small()
                            .text_color(muted.opacity(0.8)),
                    );
            } else {
                row = row
                    .on_click(cx.listener({
                        let path = path.clone();
                        move |this, _event, window, cx| {
                            window.focus(&this.focus_handle);
                            this.cancel_rename_unless(&path);
                            this.selected_path = Some(path.clone());
                            if is_layout {
                                cx.emit(DashboardManagerEvent::OpenLayout(path.clone()));
                            } else {
                                cx.emit(DashboardManagerEvent::OpenFile(path.clone()));
                            }
                        }
                    }))
                    .child(
                        Icon::default()
                            .path(if is_layout {
                                "icons/layout-dashboard.svg"
                            } else {
                                "icons/file.svg"
                            })
                            .small()
                            .text_color(if is_active {
                                colors.primary
                            } else {
                                muted.opacity(0.8)
                            }),
                    );
            }

            if let Some(input) = rename_input {
                row = row.child(
                    div()
                        .id(ElementId::Name(SharedString::from(format!(
                            "rename-input-{path}"
                        ))))
                        .flex_1()
                        .min_w_0()
                        .h(px(20.0))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(Input::new(&input).appearance(false).xsmall().w_full()),
                );
            } else {
                let extension = if is_folder {
                    String::new()
                } else if path.ends_with(LAYOUT_EXTENSION) {
                    LAYOUT_EXTENSION.to_string()
                } else {
                    Path::new(&path)
                        .extension()
                        .map(|ext| format!(".{}", ext.to_string_lossy()))
                        .unwrap_or_default()
                };
                row = row.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.0))
                        .font_weight(if is_active {
                            FontWeight::MEDIUM
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_color(if is_active {
                            foreground
                        } else {
                            foreground.opacity(0.84)
                        })
                        .child(name.clone())
                        .when(!extension.is_empty(), |this| {
                            this.child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(10.0))
                                    .text_color(muted.opacity(0.55))
                                    .child(extension.clone()),
                            )
                        }),
                );
            }

            if is_open && !is_active {
                row = row.child(
                    div()
                        .flex_shrink_0()
                        .w(px(4.0))
                        .h(px(4.0))
                        .rounded_full()
                        .bg(muted.opacity(0.55)),
                );
            } else {
                row = row.child(div().flex_shrink_0().w(px(4.0)).h(px(4.0)));
            }

            if is_folder {
                rows.push(
                    row.context_menu(folder_context_menu(
                        panel,
                        path.clone(),
                        name,
                        entry.layout_count,
                        targets,
                    ))
                    .into_any_element(),
                );
                if expanded {
                    rows.extend(self.render_entries(
                        &entry.children,
                        depth + 1,
                        filtering,
                        colors,
                        cx,
                    ));
                }
            } else if is_layout {
                rows.push(
                    row.context_menu(layout_context_menu(panel, path, name, is_open, targets))
                        .into_any_element(),
                );
            } else {
                rows.push(
                    row.context_menu(file_context_menu(panel, path, name, targets))
                        .into_any_element(),
                );
            }
        }

        rows
    }
}

impl EventEmitter<DashboardManagerEvent> for DashboardManager {}

impl Focusable for DashboardManager {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ──────────────────────────────────────────────
// Tree helpers
// ──────────────────────────────────────────────

fn find_entry<'a>(entries: &'a [LayoutEntry], path: &str) -> Option<&'a LayoutEntry> {
    for entry in entries {
        if entry.path == path {
            return Some(entry);
        }
        if let Some(found) = find_entry(&entry.children, path) {
            return Some(found);
        }
    }
    None
}

fn parent_of(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
        .unwrap_or_default()
}

fn ancestors_of(path: &str) -> Vec<String> {
    let mut ancestors = Vec::new();
    let mut current = path;
    while let Some((parent, _)) = current.rsplit_once('/') {
        ancestors.push(parent.to_string());
        current = parent;
    }
    ancestors
}

fn filter_entries(entries: &[LayoutEntry], filter: &str) -> Vec<LayoutEntry> {
    if filter.is_empty() {
        return entries.to_vec();
    }
    let needle = filter.to_lowercase();
    entries
        .iter()
        .filter_map(|entry| {
            let self_matches = entry.name.to_lowercase().contains(&needle);
            match entry.kind {
                LayoutEntryKind::Folder => {
                    let children = filter_entries(&entry.children, filter);
                    if self_matches || !children.is_empty() {
                        let mut entry = entry.clone();
                        entry.children = children;
                        Some(entry)
                    } else {
                        None
                    }
                }
                LayoutEntryKind::Layout | LayoutEntryKind::File => {
                    self_matches.then(|| entry.clone())
                }
            }
        })
        .collect()
}

fn collect_rows(
    entries: &[LayoutEntry],
    depth: usize,
    expanded: &HashSet<String>,
    filtering: bool,
    out: &mut Vec<(String, String, LayoutEntryKind, usize)>,
) {
    for entry in entries {
        out.push((entry.path.clone(), entry.name.clone(), entry.kind, depth));
        if matches!(entry.kind, LayoutEntryKind::Folder)
            && (filtering || expanded.contains(&entry.path))
        {
            collect_rows(&entry.children, depth + 1, expanded, filtering, out);
        }
    }
}

fn menu_handler<F>(
    panel: WeakEntity<DashboardManager>,
    make_event: F,
) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static
where
    F: Fn() -> DashboardManagerEvent + 'static,
{
    move |_event, _window, app| {
        if let Some(panel) = panel.upgrade() {
            app.update_entity(&panel, |_this, cx| cx.emit(make_event()));
        }
    }
}

fn begin_rename_handler(
    panel: WeakEntity<DashboardManager>,
    path: String,
) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static {
    move |_event, _window, app| {
        if let Some(panel) = panel.upgrade() {
            app.update_entity(&panel, |this, cx| this.begin_rename_path(path.clone(), cx));
        }
    }
}

fn copy_path_handler(path: String) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static {
    move |_event, _window, app| {
        app.write_to_clipboard(ClipboardItem::new_string(path.clone()));
    }
}

fn add_move_items(
    menu: PopupMenu,
    panel: WeakEntity<DashboardManager>,
    source: String,
    targets: Vec<(String, String, usize)>,
) -> PopupMenu {
    if targets.is_empty() {
        return menu.item(PopupMenuItem::new("Move To… (no other folders)").disabled(true));
    }

    let mut menu = menu;
    for (dest, label, _depth) in targets {
        let panel = panel.clone();
        let source = source.clone();
        let dest_for_event = dest.clone();
        let label = if dest.is_empty() {
            "Move to Workspace root".to_string()
        } else {
            format!("Move to \"{label}\"")
        };
        menu = menu.item(
            PopupMenuItem::new(label)
                .icon(Icon::default().path("icons/folder-open.svg"))
                .on_click(menu_handler(panel, move || {
                    DashboardManagerEvent::MoveEntry {
                        path: source.clone(),
                        dest: dest_for_event.clone(),
                    }
                })),
        );
    }
    menu
}

fn layout_context_menu(
    panel: WeakEntity<DashboardManager>,
    path: String,
    name: String,
    is_open: bool,
    targets: Vec<(String, String, usize)>,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    move |menu, _window, _cx| {
        let mut menu = menu;

        menu = menu.item(
            PopupMenuItem::new("Open")
                .icon(Icon::default().path("icons/file.svg"))
                .on_click(menu_handler(panel.clone(), {
                    let path = path.clone();
                    move || DashboardManagerEvent::OpenLayout(path.clone())
                })),
        );
        menu = menu.item(
            PopupMenuItem::new("Open as Text")
                .icon(Icon::default().path("icons/file.svg"))
                .on_click(menu_handler(panel.clone(), {
                    let path = path.clone();
                    move || DashboardManagerEvent::OpenFile(path.clone())
                })),
        );
        menu = menu.item(
            PopupMenuItem::new("Open in New Window")
                .icon(Icon::default().path("icons/external-link.svg"))
                .on_click(menu_handler(panel.clone(), {
                    let path = path.clone();
                    move || DashboardManagerEvent::DetachLayout(path.clone())
                })),
        );
        if is_open {
            menu = menu.item(
                PopupMenuItem::new("Close Tab")
                    .icon(Icon::default().path("icons/close.svg"))
                    .on_click(menu_handler(panel.clone(), {
                        let path = path.clone();
                        move || DashboardManagerEvent::CloseLayout(path.clone())
                    })),
            );
        }

        menu = menu.separator();
        menu = menu.item(
            PopupMenuItem::new("Rename…")
                .icon(Icon::default().path("icons/pencil.svg"))
                .on_click(begin_rename_handler(panel.clone(), path.clone())),
        );
        menu = menu.item(
            PopupMenuItem::new("Duplicate")
                .icon(Icon::default().path("icons/copy.svg"))
                .on_click(menu_handler(panel.clone(), {
                    let path = path.clone();
                    move || DashboardManagerEvent::DuplicateEntry(path.clone())
                })),
        );
        menu = add_move_items(menu, panel.clone(), path.clone(), targets.clone());

        menu = menu.separator();
        menu = menu.item(
            PopupMenuItem::new("Reveal in File Manager")
                .icon(Icon::default().path("icons/folder-open.svg"))
                .on_click(menu_handler(panel.clone(), {
                    let path = path.clone();
                    move || DashboardManagerEvent::RevealEntry(path.clone())
                })),
        );
        menu = menu.item(
            PopupMenuItem::new("Copy Relative Path")
                .icon(Icon::default().path("icons/copy.svg"))
                .on_click(copy_path_handler(path.clone())),
        );

        menu.separator().item(
            PopupMenuItem::new(format!("Delete \"{name}\"…"))
                .icon(Icon::default().path("icons/delete.svg"))
                .on_click(menu_handler(panel.clone(), {
                    let path = path.clone();
                    let name = name.clone();
                    move || DashboardManagerEvent::DeleteEntry {
                        path: path.clone(),
                        name: name.clone(),
                        is_folder: false,
                        child_count: 0,
                    }
                })),
        )
    }
}

fn file_context_menu(
    panel: WeakEntity<DashboardManager>,
    path: String,
    name: String,
    targets: Vec<(String, String, usize)>,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    move |menu, _window, _cx| {
        let mut menu = menu;

        menu = menu.item(
            PopupMenuItem::new("Open")
                .icon(Icon::default().path("icons/file.svg"))
                .on_click(menu_handler(panel.clone(), {
                    let path = path.clone();
                    move || DashboardManagerEvent::OpenFile(path.clone())
                })),
        );

        menu = menu.separator();
        menu = menu.item(
            PopupMenuItem::new("Rename…")
                .icon(Icon::default().path("icons/pencil.svg"))
                .on_click(begin_rename_handler(panel.clone(), path.clone())),
        );
        menu = menu.item(
            PopupMenuItem::new("Duplicate")
                .icon(Icon::default().path("icons/copy.svg"))
                .on_click(menu_handler(panel.clone(), {
                    let path = path.clone();
                    move || DashboardManagerEvent::DuplicateEntry(path.clone())
                })),
        );
        menu = add_move_items(menu, panel.clone(), path.clone(), targets.clone());

        menu = menu.separator();
        menu = menu.item(
            PopupMenuItem::new("Reveal in File Manager")
                .icon(Icon::default().path("icons/folder-open.svg"))
                .on_click(menu_handler(panel.clone(), {
                    let path = path.clone();
                    move || DashboardManagerEvent::RevealEntry(path.clone())
                })),
        );
        menu = menu.item(
            PopupMenuItem::new("Copy Relative Path")
                .icon(Icon::default().path("icons/copy.svg"))
                .on_click(copy_path_handler(path.clone())),
        );

        menu.separator().item(
            PopupMenuItem::new(format!("Delete \"{name}\"…"))
                .icon(Icon::default().path("icons/delete.svg"))
                .on_click(menu_handler(panel.clone(), {
                    let path = path.clone();
                    let name = name.clone();
                    move || DashboardManagerEvent::DeleteEntry {
                        path: path.clone(),
                        name: name.clone(),
                        is_folder: false,
                        child_count: 0,
                    }
                })),
        )
    }
}

fn folder_context_menu(
    panel: WeakEntity<DashboardManager>,
    path: String,
    name: String,
    child_count: usize,
    targets: Vec<(String, String, usize)>,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    move |menu, _window, _cx| {
        let mut menu = menu;

        menu = menu.item(
            PopupMenuItem::new("New File")
                .icon(Icon::default().path("icons/plus.svg"))
                .on_click(menu_handler(panel.clone(), {
                    let path = path.clone();
                    move || DashboardManagerEvent::NewFile(path.clone())
                })),
        );
        menu = menu.item(
            PopupMenuItem::new("New Layout")
                .icon(Icon::default().path("icons/layout-dashboard.svg"))
                .on_click(menu_handler(panel.clone(), {
                    let path = path.clone();
                    move || DashboardManagerEvent::NewLayout(path.clone())
                })),
        );
        menu = menu.item(
            PopupMenuItem::new("New Folder")
                .icon(Icon::default().path("icons/folder.svg"))
                .on_click(menu_handler(panel.clone(), {
                    let path = path.clone();
                    move || DashboardManagerEvent::NewFolder(path.clone())
                })),
        );

        menu = menu.separator();
        menu = menu.item(
            PopupMenuItem::new("Rename…")
                .icon(Icon::default().path("icons/pencil.svg"))
                .on_click(begin_rename_handler(panel.clone(), path.clone())),
        );
        menu = add_move_items(menu, panel.clone(), path.clone(), targets.clone());

        menu = menu.separator();
        menu = menu.item(
            PopupMenuItem::new("Reveal in File Manager")
                .icon(Icon::default().path("icons/folder-open.svg"))
                .on_click(menu_handler(panel.clone(), {
                    let path = path.clone();
                    move || DashboardManagerEvent::RevealEntry(path.clone())
                })),
        );
        menu = menu.item(
            PopupMenuItem::new("Copy Relative Path")
                .icon(Icon::default().path("icons/copy.svg"))
                .on_click(copy_path_handler(path.clone())),
        );

        menu.separator().item(
            PopupMenuItem::new(format!("Delete Folder \"{name}\"…"))
                .icon(Icon::default().path("icons/delete.svg"))
                .on_click(menu_handler(panel.clone(), {
                    let path = path.clone();
                    let name = name.clone();
                    move || DashboardManagerEvent::DeleteEntry {
                        path: path.clone(),
                        name: name.clone(),
                        is_folder: true,
                        child_count,
                    }
                })),
        )
    }
}

fn workspace_context_menu(
    panel: WeakEntity<DashboardManager>,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    move |menu, _window, _cx| {
        let mut menu = menu;
        menu = menu.item(
            PopupMenuItem::new("New File")
                .icon(Icon::default().path("icons/plus.svg"))
                .on_click(menu_handler(panel.clone(), || {
                    DashboardManagerEvent::NewFile(String::new())
                })),
        );
        menu = menu.item(
            PopupMenuItem::new("New Layout")
                .icon(Icon::default().path("icons/layout-dashboard.svg"))
                .on_click(menu_handler(panel.clone(), || {
                    DashboardManagerEvent::NewLayout(String::new())
                })),
        );
        menu = menu.item(
            PopupMenuItem::new("New Folder")
                .icon(Icon::default().path("icons/folder.svg"))
                .on_click(menu_handler(panel.clone(), || {
                    DashboardManagerEvent::NewFolder(String::new())
                })),
        );
        menu = menu.separator();
        menu = menu.item(
            PopupMenuItem::new("Open Folder…")
                .icon(Icon::default().path("icons/folder-open.svg"))
                .on_click(menu_handler(panel.clone(), || {
                    DashboardManagerEvent::OpenWorkspace
                })),
        );
        menu = menu.item(
            PopupMenuItem::new("Reveal Workspace")
                .icon(Icon::default().path("icons/external-link.svg"))
                .on_click(menu_handler(panel.clone(), || {
                    DashboardManagerEvent::RevealEntry(String::new())
                })),
        );
        menu = menu.separator();
        menu.item(
            PopupMenuItem::new("Refresh")
                .icon(Icon::default().path("icons/refresh-cw.svg"))
                .on_click(menu_handler(panel.clone(), || {
                    DashboardManagerEvent::Refresh
                })),
        )
    }
}

// ──────────────────────────────────────────────
// Rendering
// ──────────────────────────────────────────────

fn row_shell(
    id: &str,
    depth: usize,
    is_active: bool,
    is_selected: bool,
    hover_bg: Hsla,
    active_bg: Hsla,
    selected_bg: Hsla,
) -> Stateful<gpui::Div> {
    let group: SharedString = SharedString::from(format!("tree-row-{id}"));

    div()
        .id(ElementId::Name(SharedString::from(format!(
            "tree-row-{id}"
        ))))
        .group(group)
        .h(px(ROW_HEIGHT))
        .w_full()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(6.0))
        .pl(px(8.0 + INDENT * depth as f32))
        .pr(px(6.0))
        .rounded(px(4.0))
        .cursor_pointer()
        .when(is_active, |this| this.bg(active_bg))
        .when(!is_active && is_selected, |this| this.bg(selected_bg))
        .when(!is_active && !is_selected, |this| {
            this.hover(move |s| s.bg(hover_bg))
        })
}

impl Render for DashboardManager {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_filter_input(window, cx);
        self.ensure_rename_input(window, cx);
        if let Some(input) = &self.rename_input {
            let handle = input.read(cx).focus_handle(cx);
            if !handle.is_focused(window) {
                handle.focus(window);
            }
        }

        let colors = cx.theme().colors;
        let hover_bg = colors.foreground.opacity(0.05);
        let muted = colors.muted_foreground;
        let foreground = colors.foreground;
        let filtering = !self.filter.is_empty();

        let entries = self.filtered_entries();
        let rows = self.render_entries(&entries, 0, filtering, &colors, cx);
        let root_count: usize = self.entries.iter().map(|entry| entry.layout_count).sum();
        let root_panel = cx.entity().downgrade();
        let empty_panel = cx.entity().downgrade();
        let filter_input = self.filter_input.clone();

        let tree = div()
            .id("layout-explorer-tree")
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .py(px(4.0))
            .px(px(6.0))
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                if this.rename_path.is_some() {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "up" => {
                        this.select_delta(-1, cx);
                        cx.stop_propagation();
                    }
                    "down" => {
                        this.select_delta(1, cx);
                        cx.stop_propagation();
                    }
                    "left" => {
                        this.collapse_selected_or_parent(cx);
                        cx.stop_propagation();
                    }
                    "right" => {
                        this.expand_or_open_selected(cx);
                        cx.stop_propagation();
                    }
                    "enter" => {
                        this.activate_selected(cx);
                        cx.stop_propagation();
                    }
                    "f2" => {
                        this.begin_rename_selected(cx);
                        cx.stop_propagation();
                    }
                    "delete" => {
                        this.delete_selected(cx);
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .child(
                div()
                    .id("workspace-root-row")
                    .h(px(26.0))
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(6.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover_bg))
                    .on_click(cx.listener(|this, _event, window, cx| {
                        window.focus(&this.focus_handle);
                        this.rename_path = None;
                        this.rename_input = None;
                        this.root_expanded = !this.root_expanded;
                        cx.notify();
                    }))
                    .context_menu(workspace_context_menu(root_panel))
                    .child(
                        Icon::default()
                            .path(if self.root_expanded {
                                "icons/folder-open.svg"
                            } else {
                                "icons/folder-closed.svg"
                            })
                            .small()
                            .text_color(muted.opacity(0.8)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(foreground.opacity(0.9))
                            .child(self.root_name.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.0))
                            .text_color(muted.opacity(0.55))
                            .child(root_count.to_string()),
                    ),
            )
            .when(self.root_expanded, |this| this.children(rows))
            .when(entries.is_empty(), |this| {
                this.child(
                    div()
                        .pl(px(26.0))
                        .pr(px(8.0))
                        .py(px(8.0))
                        .text_size(px(11.0))
                        .text_color(muted.opacity(0.85))
                        .child(if filtering {
                            "No matching layouts"
                        } else {
                            "This folder is empty. Right-click to create a layout."
                        }),
                )
            })
            .child(
                // Blank space below the tree opens workspace actions.
                div()
                    .id("layout-explorer-empty-space")
                    .flex_1()
                    .min_h(px(32.0))
                    .context_menu(workspace_context_menu(empty_panel)),
            );

        div()
            .id("layout-explorer")
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(colors.sidebar)
            .text_color(foreground)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .h(px(32.0))
                    .px(px(6.0))
                    .flex_shrink_0()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h(px(24.0))
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .pl(px(6.0))
                            .rounded(px(5.0))
                            .bg(colors.foreground.opacity(0.05))
                            .border_1()
                            .border_color(colors.border.opacity(0.6))
                            .child(
                                Icon::default()
                                    .path("icons/search.svg")
                                    .xsmall()
                                    .text_color(muted.opacity(0.7)),
                            )
                            .child(div().flex_1().min_w_0().h_full().when_some(
                                filter_input,
                                |this, input| {
                                    this.child(
                                        Input::new(&input)
                                            .appearance(false)
                                            .cleanable(true)
                                            .xsmall(),
                                    )
                                },
                            )),
                    )
                    .child(
                        Button::new("explorer-open-folder")
                            .icon(Icon::default().path("icons/folder-open.svg"))
                            .ghost()
                            .xsmall()
                            .tooltip("Open layouts folder")
                            .on_click(cx.listener(|_this, _event, _window, cx| {
                                cx.emit(DashboardManagerEvent::OpenWorkspace);
                            })),
                    )
                    .child(
                        Button::new("explorer-new-file")
                            .icon(Icon::default().path("icons/plus.svg"))
                            .ghost()
                            .xsmall()
                            .tooltip("New file")
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.new_file(cx);
                            })),
                    )
                    .child(
                        Button::new("explorer-more")
                            .icon(Icon::default().path("icons/ellipsis.svg"))
                            .ghost()
                            .xsmall()
                            .tooltip("Workspace actions")
                            .dropdown_menu_with_anchor(
                                gpui::Corner::BottomRight,
                                workspace_context_menu(cx.entity().downgrade()),
                            ),
                    ),
            )
            .child(tree)
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .mx(px(8.0))
                        .mb(px(8.0))
                        .p(px(8.0))
                        .min_w_0()
                        .rounded(px(4.0))
                        .bg(colors.danger.opacity(0.1))
                        .border_1()
                        .border_color(colors.danger.opacity(0.25))
                        .flex()
                        .items_start()
                        .gap(px(6.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(11.0))
                                .text_color(colors.danger)
                                .child(error),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .cursor_pointer()
                                .text_size(px(11.0))
                                .text_color(colors.danger.opacity(0.8))
                                .hover(|s| s.text_color(colors.danger))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| this.clear_error(cx)),
                                )
                                .child("✕"),
                        ),
                )
            })
    }
}
