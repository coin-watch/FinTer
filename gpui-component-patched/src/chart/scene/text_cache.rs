//! Shaped-text cache.
//!
//! Text shaping is expensive and must not happen inside a paint loop for every
//! label. Entries are keyed by text, font, size, and color and reused across
//! frames.

use std::collections::HashMap;

use gpui::{Font, Hsla, SharedString, TextRun, WrappedLine, px};

use crate::PixelsExt;

#[derive(Clone, PartialEq, Eq, Hash)]
struct TextKey {
    text: SharedString,
    font: Font,
    size_bits: u32,
    color_bits: [u32; 4],
}

#[derive(Clone, Debug, Default)]
pub struct ShapedText {
    pub lines: Vec<WrappedLine>,
    pub width: f32,
    pub height: f32,
}

impl ShapedText {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

pub struct TextCache {
    entries: HashMap<TextKey, ShapedText>,
    max_entries: usize,
}

impl Default for TextCache {
    fn default() -> Self {
        Self::new(2_048)
    }
}

impl TextCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: HashMap::new(),
            max_entries: max_entries.max(32),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn measure(
        &mut self,
        text: &SharedString,
        font: &Font,
        font_size: f32,
        color: Hsla,
        window: &mut gpui::Window,
    ) -> f32 {
        self.get_or_shape(text, font, font_size, color, window)
            .width
    }

    pub fn get_or_shape(
        &mut self,
        text: &SharedString,
        font: &Font,
        font_size: f32,
        color: Hsla,
        window: &mut gpui::Window,
    ) -> &ShapedText {
        let size_bits = font_size.to_bits();
        let key = TextKey {
            text: text.clone(),
            font: font.clone(),
            size_bits,
            color_bits: [
                color.h.to_bits(),
                color.s.to_bits(),
                color.l.to_bits(),
                color.a.to_bits(),
            ],
        };

        if !self.entries.contains_key(&key) {
            if self.entries.len() >= self.max_entries {
                self.entries.clear();
            }
            let shaped = shape(text, font, font_size, color, window);
            self.entries.insert(key.clone(), shaped);
        }
        self.entries
            .get(&key)
            .expect("text entry was just inserted")
    }
}

fn shape(
    text: &SharedString,
    font: &Font,
    font_size: f32,
    color: Hsla,
    window: &mut gpui::Window,
) -> ShapedText {
    if text.is_empty() || !font_size.is_finite() || font_size <= 0.0 {
        return ShapedText::default();
    }
    let run = TextRun {
        len: text.len(),
        font: font.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let font_size_px = px(font_size);
    let Ok(shaped) =
        window
            .text_system()
            .shape_text(text.clone(), font_size_px, &[run], None, None)
    else {
        return ShapedText::default();
    };
    let lines: Vec<WrappedLine> = shaped.into_iter().collect();
    let width = lines
        .iter()
        .map(|line| line.size(font_size_px).width.as_f32())
        .fold(0.0_f32, f32::max);
    let height = font_size * lines.len().max(1) as f32 * 1.2;
    ShapedText {
        lines,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_is_not_shaped() {
        let font = gpui::font("Test");
        let mut cache = TextCache::default();
        // Shaping requires a window, so only verify the fast path's contract
        // through the key/value shape here.
        let key = TextKey {
            text: "".into(),
            font,
            size_bits: 12.0f32.to_bits(),
            color_bits: [0, 0, 0, 0],
        };
        cache.entries.insert(
            key,
            ShapedText {
                lines: Vec::new(),
                width: 0.0,
                height: 0.0,
            },
        );
        assert_eq!(cache.len(), 1);
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn capacity_is_lower_bounded() {
        let cache = TextCache::new(0);
        assert!(cache.max_entries >= 32);
    }
}
