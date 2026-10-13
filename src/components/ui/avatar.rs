use gpui::*;
use gpui_component::{Sizable, avatar::Avatar};

pub fn build_avatar_element(
    name: SharedString,
    size: Option<gpui_component::Size>,
    src: Option<SharedString>,
) -> impl IntoElement {
    let mut avatar = Avatar::new().name(name);
    // .with_size(px(32.));

    if let Some(size) = size {
        avatar = avatar.with_size(size);
    }
    if let Some(src) = &src {
        avatar = avatar.src(src.clone());
    }

    div().child(avatar)
}
