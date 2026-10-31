//! Whether the pointer and the keyboard belong to the scene or to the UI:
//! the bar, its lists and the side panel.
//!
//! The camera reads [`UiFocus`] before it answers the wheel or a key: the
//! wheel over the bar or the panel scrolls their lists instead of changing
//! the camera's speed, and while a text field in the bar takes the keyboard
//! no key moves the camera or changes the view. A click on either never
//! reaches the scene beneath, and nothing under them is hovered.
//! Everything else reaches the scene whatever the UI is doing: a right drag
//! looks around wherever it starts, since the UI has no use for the right
//! button.
//!
//! The bar (`bar.rs`) keeps [`UiFocus`] up to date each update, after
//! picking has worked out what is under the pointer, counting every UI
//! node that blocks what lies beneath, the panel's among them; without the
//! bar it stays at its default, which gives everything to the scene.

use bevy::prelude::*;

/// Who the pointer and the keyboard belong to.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiFocus {
    /// The mouse pointer is over a UI node, which blocks the scene beneath.
    pub pointer_over_ui: bool,
    /// A text field has the keyboard.
    pub typing: bool,
}

/// Whether any of the entities under the pointer is a UI node.
pub fn over_ui(hovered: impl IntoIterator<Item = Entity>, is_ui: impl Fn(Entity) -> bool) -> bool {
    hovered.into_iter().any(is_ui)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pointer_is_over_the_ui_when_any_hovered_entity_is_a_node() {
        let world = &mut World::new();
        let node = world.spawn_empty().id();
        let disc = world.spawn_empty().id();
        let is_ui = |e: Entity| e == node;
        assert!(over_ui([disc, node], is_ui));
        assert!(!over_ui([disc], is_ui));
        assert!(!over_ui([], is_ui));
    }
}
