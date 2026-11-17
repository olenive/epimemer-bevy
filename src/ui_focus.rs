//! Whether the pointer and the keyboard belong to the scene or to the UI:
//! the bar, its lists and the side panel.
//!
//! The camera reads [`UiFocus`] before it answers the wheel, a left press or
//! a key: the wheel over the bar or the panel scrolls their lists instead
//! of zooming the camera or changing its speed, a left drag that begins
//! over them never pans, since the bar's slider has its own use for one,
//! and while a text field in the bar takes the keyboard no key moves the
//! camera or changes the view. A click on either never reaches the scene
//! beneath, and nothing under them is hovered. Everything else reaches the
//! scene whatever the UI is doing: a right drag looks around wherever it
//! starts, since the UI has no use for the right button.
//!
//! The bar (`bar.rs`) keeps [`UiFocus`] up to date each update, after
//! picking has worked out what is under the pointer, counting every UI
//! node that blocks what lies beneath, the panel's among them; without the
//! bar it stays at its default, which gives everything to the scene.
//!
//! Over the words of a text made of spans, such as a row of the side panel
//! that names a node, Bevy's UI picking reports the span's own entity,
//! which is not a node, and the span blocks what lies beneath as its text
//! does. So an entity counts as UI when it or any entity it belongs to is a
//! UI node ([`is_ui`]), and the pointer anywhere over the bar, its lists or
//! the panel belongs to the UI, words and gaps alike.

use bevy::prelude::*;

/// Who the pointer and the keyboard belong to.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiFocus {
    /// The mouse pointer is over a UI node, which blocks the scene beneath.
    pub pointer_over_ui: bool,
    /// A text field has the keyboard.
    pub typing: bool,
}

/// Whether any of the entities under the pointer is UI ([`is_ui`]).
pub fn over_ui(hovered: impl IntoIterator<Item = Entity>, is_ui: impl Fn(Entity) -> bool) -> bool {
    hovered.into_iter().any(is_ui)
}

/// Whether an entity is part of the UI: a UI node (`is_node`), or a part of
/// one that is not a node itself, such as a span of a text, found by
/// climbing from each entity to its `parent`.
pub fn is_ui(
    entity: Entity,
    is_node: impl Fn(Entity) -> bool,
    parent: impl Fn(Entity) -> Option<Entity>,
) -> bool {
    std::iter::successors(Some(entity), |&e| parent(e)).any(is_node)
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

    #[test]
    fn a_span_of_a_texts_words_is_ui_as_its_text_is_and_a_disc_is_not() {
        let world = &mut World::new();
        let row = world.spawn(Node::default()).id();
        let text = world.spawn((Text::default(), ChildOf(row))).id();
        let span = world.spawn((TextSpan::new("words"), ChildOf(text))).id();
        let disc = world.spawn_empty().id();
        let is_node = |e: Entity| world.get::<ComputedNode>(e).is_some();
        let parent = |e: Entity| world.get::<ChildOf>(e).map(ChildOf::parent);
        assert!(!is_node(span), "a span is no node of its own");
        for entity in [row, text, span] {
            assert!(is_ui(entity, is_node, parent), "{entity:?}");
        }
        assert!(!is_ui(disc, is_node, parent));
        assert!(over_ui([span], |e| is_ui(e, is_node, parent)));
    }
}
