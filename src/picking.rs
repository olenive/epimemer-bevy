//! Picking: which disc the pointer is on, through Bevy's mesh picking
//! backend (DESIGN.md §8). What a hover and a click do is `focus.rs`.
//!
//! Every disc takes part while the filter leaves it visible: a node the
//! filter hides can be neither hovered nor clicked. Picking is opt-in per
//! entity, so a hidden disc costs the ray cast nothing.
//!
//! The bar along the top (`bar.rs`) is UI, and Bevy's UI picking backend
//! ranks UI nodes above every mesh, with each node blocking what lies
//! under it, so a click on the bar never reaches a disc beneath it. The
//! labels (`labels.rs`) are UI too, and take no part in picking, so a disc
//! under a label can still be hovered and clicked.

use bevy::prelude::*;

use crate::filter::{RefreshShown, Shown, is_visible};
use crate::scene::Disc;

pub struct PickPlugin;

impl Plugin for PickPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MeshPickingPlugin)
            .insert_resource(MeshPickingSettings {
                require_markers: true,
                ..default()
            })
            .add_systems(PostStartup, mark_camera)
            .add_systems(PostUpdate, sync_pickable.after(RefreshShown));
    }
}

/// Marks the camera for the mesh picking backend, once it is spawned.
fn mark_camera(mut commands: Commands, camera: Single<Entity, With<Camera3d>>) {
    commands.entity(*camera).insert(MeshPickingCamera);
}

/// Gives each disc the picking marker while it is visible and takes it
/// away while it is hidden, in any update in which [`Shown`] changed.
fn sync_pickable(
    mut commands: Commands,
    shown: Res<Shown>,
    discs: Query<(Entity, &Disc, Has<Pickable>)>,
) {
    if !shown.is_changed() {
        return;
    }
    for (entity, disc, pickable) in &discs {
        let wanted = is_visible(&shown.visible, disc.index);
        match (wanted, pickable) {
            (true, false) => {
                commands.entity(entity).insert(Pickable::default());
            }
            (false, true) => {
                commands.entity(entity).remove::<Pickable>();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::Mode;
    use crate::layout::Kind;

    #[test]
    fn every_disc_takes_part_while_visible_and_leaves_while_hidden() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(Shown::default())
            .add_systems(PostUpdate, sync_pickable);
        let kinds = [Kind::Topic, Kind::Topic, Kind::Fact];
        let discs: Vec<Entity> = kinds
            .iter()
            .enumerate()
            .map(|(index, &kind)| app.world_mut().spawn(Disc { index, kind }).id())
            .collect();
        let pickable = |app: &App| -> Vec<bool> {
            discs
                .iter()
                .map(|&e| app.world().get::<Pickable>(e).is_some())
                .collect()
        };
        app.update();
        assert_eq!(pickable(&app), [true, true, true], "claims too");

        app.insert_resource(Shown {
            mode: Mode::Hide,
            passes: vec![true, false, false],
            visible: vec![true, false, false],
        });
        app.update();
        assert_eq!(pickable(&app), [true, false, false]);

        app.world_mut().resource_mut::<Shown>().visible = vec![true; 3];
        app.update();
        assert_eq!(pickable(&app), [true, true, true]);
    }
}
