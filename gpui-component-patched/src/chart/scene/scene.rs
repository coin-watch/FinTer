//! Chart scene: a display list grouped into z-order layers.

use super::layer::Layer;
use super::primitive::Primitive;

#[derive(Clone, Debug, Default)]
pub struct Scene {
    layers: [Vec<Primitive>; Layer::COUNT],
}

impl Scene {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        for layer in &mut self.layers {
            layer.clear();
        }
    }

    pub fn push(&mut self, layer: Layer, primitive: Primitive) {
        self.layers[layer.index()].push(primitive);
    }

    pub fn extend(&mut self, layer: Layer, primitives: impl IntoIterator<Item = Primitive>) {
        self.layers[layer.index()].extend(primitives);
    }

    pub fn layer(&self, layer: Layer) -> &[Primitive] {
        &self.layers[layer.index()]
    }

    pub fn layer_mut(&mut self, layer: Layer) -> &mut Vec<Primitive> {
        &mut self.layers[layer.index()]
    }

    pub fn primitive_count(&self) -> usize {
        self.layers.iter().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.layers.iter().all(Vec::is_empty)
    }

    /// Reuse allocated buffers when rebuilding the scene.
    pub fn reset(&mut self) {
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::core::Rect;
    use gpui::hsla;

    #[test]
    fn layers_are_painted_in_order() {
        let mut scene = Scene::new();
        scene.push(
            Layer::Cursor,
            Primitive::rect(Rect::new(0.0, 0.0, 1.0, 1.0), hsla(0.0, 0.0, 1.0, 1.0)),
        );
        scene.push(
            Layer::Grid,
            Primitive::rect(Rect::new(1.0, 1.0, 1.0, 1.0), hsla(0.0, 0.0, 0.5, 1.0)),
        );
        assert_eq!(scene.layer(Layer::Grid).len(), 1);
        assert_eq!(scene.layer(Layer::Cursor).len(), 1);
        assert_eq!(scene.primitive_count(), 2);
    }

    #[test]
    fn clear_preserves_layer_structure() {
        let mut scene = Scene::new();
        scene.push(
            Layer::Series,
            Primitive::rect(Rect::new(0.0, 0.0, 1.0, 1.0), hsla(0.0, 0.0, 1.0, 1.0)),
        );
        scene.clear();
        assert!(scene.is_empty());
        assert_eq!(scene.layer(Layer::Series).len(), 0);
    }
}
