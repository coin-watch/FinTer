//! Generic local-file viewer/editor used when the explorer opens a file that
//! is not a FinTer layout.
//!
//! Text files are editable with the code editor component and saved back to
//! disk. Images are previewed. Other binary files show their size instead of a
//! meaningless text dump.

use crate::data::layout_workspace;
use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render, Styled, StyledImage,
    Window, div, img, prelude::FluentBuilder, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme, Icon, Sizable};
use std::path::{Path, PathBuf};

pub enum FileEditorEvent {
    Close,
}

pub struct FileEditor {
    focus_handle: FocusHandle,
    path: String,
    absolute: PathBuf,
    title: String,
    text: Option<String>,
    input: Option<Entity<InputState>>,
    error: Option<String>,
    save_error: Option<String>,
    binary_size: Option<u64>,
    is_image: bool,
    dirty: bool,
}

impl FileEditor {
    pub fn new(path: String, cx: &mut Context<Self>) -> Self {
        let absolute =
            layout_workspace::absolute_path(&path).unwrap_or_else(|_| PathBuf::from(&path));
        let title = Path::new(&path)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());
        let is_image = is_image_path(&path);

        let mut editor = Self {
            focus_handle: cx.focus_handle(),
            path,
            absolute,
            title,
            text: None,
            input: None,
            error: None,
            save_error: None,
            binary_size: None,
            is_image,
            dirty: false,
        };
        editor.load();
        editor
    }

    /// Re-read the file from disk and discard the current input state. Used
    /// when the Agent writes to a file that is already open in a tab.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.absolute = layout_workspace::absolute_path(&self.path)
            .unwrap_or_else(|_| PathBuf::from(&self.path));
        self.is_image = is_image_path(&self.path);
        self.input = None;
        self.dirty = false;
        self.save_error = None;
        self.load();
        cx.notify();
    }

    fn load(&mut self) {
        self.text = None;
        self.error = None;
        self.binary_size = None;

        match layout_workspace::read_bytes(&self.path) {
            Ok(bytes) => {
                if self.is_image {
                    self.binary_size = Some(bytes.len() as u64);
                } else if bytes.len() > 16 * 1024 * 1024 {
                    self.error = Some(format!(
                        "File is too large to edit here ({}).",
                        format_bytes(bytes.len() as u64)
                    ));
                } else {
                    let size = bytes.len() as u64;
                    match String::from_utf8(bytes) {
                        Ok(text) => self.text = Some(text),
                        Err(_) => self.binary_size = Some(size),
                    }
                }
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    pub fn title(&self) -> String {
        self.title.clone()
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn ensure_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.input.is_some() {
            return;
        }
        let Some(text) = self.text.clone() else {
            return;
        };
        let language = language_for(&self.path);
        let input = cx.new(|cx| {
            let state = InputState::new(window, cx).default_value(text);
            match language {
                Some(language) => state.code_editor(language),
                None => state.multi_line(true),
            }
        });
        cx.subscribe(&input, |this, _input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.dirty = true;
                this.save_error = None;
                cx.notify();
            }
        })
        .detach();
        self.input = Some(input);
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(input) = &self.input else {
            return;
        };
        let content = input.read(cx).value().to_string();
        match layout_workspace::write_text(&self.path, &content) {
            Ok(()) => {
                self.dirty = false;
                self.save_error = None;
            }
            Err(error) => self.save_error = Some(error.to_string()),
        }
        cx.notify();
    }
}

impl EventEmitter<FileEditorEvent> for FileEditor {}

impl Focusable for FileEditor {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for FileEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_input(window, cx);
        let colors = cx.theme().colors;
        let muted = colors.muted_foreground;
        let foreground = colors.foreground;
        let input = self.input.clone();

        let body: gpui::AnyElement = if let Some(error) = self.error.clone() {
            empty_message(&error, colors.danger).into_any_element()
        } else if self.is_image {
            div()
                .flex_1()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(colors.background)
                .child(
                    img(self.absolute.clone())
                        .max_w_full()
                        .max_h_full()
                        .object_fit(gpui::ObjectFit::Contain),
                )
                .into_any_element()
        } else if let Some(input) = input {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .overflow_hidden()
                .bg(colors.background)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h(px(0.0))
                        .child(Input::new(&input).appearance(false).h_full().w_full()),
                )
                .into_any_element()
        } else {
            let size = self
                .binary_size
                .map(format_bytes)
                .unwrap_or_else(|| "unknown size".to_string());
            empty_message(
                &format!("Binary file ({size}) — no text preview available."),
                muted,
            )
            .into_any_element()
        };

        let close = {
            let entity = cx.entity().downgrade();
            move |_event: &gpui::ClickEvent, _window: &mut Window, app: &mut App| {
                if let Some(entity) = entity.upgrade() {
                    app.update_entity(&entity, |_this, cx| {
                        cx.emit(FileEditorEvent::Close);
                    });
                }
            }
        };

        let save = {
            let entity = cx.entity().downgrade();
            move |_event: &gpui::ClickEvent, _window: &mut Window, app: &mut App| {
                if let Some(entity) = entity.upgrade() {
                    app.update_entity(&entity, |this, cx| this.save(cx));
                }
            }
        };

        div()
            .id("file-editor")
            .size_full()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .bg(colors.background)
            .text_color(foreground)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                if event.keystroke.key == "s" && event.keystroke.modifiers.secondary() {
                    this.save(cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .h(px(34.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(10.0))
                    .border_b_1()
                    .border_color(colors.border)
                    .bg(colors.title_bar)
                    .child(
                        Icon::default()
                            .path("icons/file.svg")
                            .small()
                            .text_color(muted),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .child(self.title.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(10.0))
                            .text_color(muted.opacity(0.65))
                            .child(self.path.clone()),
                    )
                    .when(self.dirty, |this| {
                        this.child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(10.0))
                                .text_color(muted)
                                .child("● modified"),
                        )
                    })
                    .when(self.text.is_some(), |this| {
                        this.child(
                            Button::new("file-editor-save")
                                .icon(Icon::default().path("icons/hard-drive.svg"))
                                .ghost()
                                .xsmall()
                                .tooltip("Save (Ctrl/Cmd+S)")
                                .on_click(save),
                        )
                    })
                    .child(
                        Button::new("file-editor-close")
                            .icon(Icon::default().path("icons/close.svg"))
                            .ghost()
                            .xsmall()
                            .tooltip("Close file")
                            .on_click(close),
                    ),
            )
            .when_some(self.save_error.clone(), |this, error| {
                this.child(
                    div()
                        .px(px(10.0))
                        .py(px(4.0))
                        .text_size(px(11.0))
                        .text_color(colors.danger)
                        .bg(colors.danger.opacity(0.08))
                        .child(error),
                )
            })
            .child(body)
    }
}

fn empty_message(message: &str, color: gpui::Hsla) -> impl IntoElement {
    div()
        .flex_1()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .px(px(24.0))
        .text_center()
        .text_sm()
        .text_color(color)
        .child(message.to_string())
}

fn is_image_path(path: &str) -> bool {
    matches!(
        Path::new(path)
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "svg")
    )
}

fn language_for(path: &str) -> Option<&'static str> {
    match Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("json" | "finter") => Some("json"),
        Some("md" | "markdown") => Some("markdown"),
        Some("rs") => Some("rust"),
        Some("toml") => Some("toml"),
        Some("yaml" | "yml") => Some("yaml"),
        Some("js" | "jsx" | "mjs" | "cjs") => Some("javascript"),
        Some("ts" | "tsx") => Some("typescript"),
        Some("html" | "htm") => Some("html"),
        Some("css") => Some("css"),
        Some("py") => Some("python"),
        Some("rb") => Some("ruby"),
        Some("sh" | "bash" | "zsh") => Some("bash"),
        Some("sql") => Some("sql"),
        Some("go") => Some("go"),
        Some("c" | "h") => Some("c"),
        Some("cc" | "cpp" | "cxx" | "hpp") => Some("cpp"),
        Some("java") => Some("java"),
        Some("zig") => Some("zig"),
        Some("xml" | "svg") => Some("xml"),
        _ => None,
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
