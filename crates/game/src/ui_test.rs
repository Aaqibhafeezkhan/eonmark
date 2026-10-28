//! Test-only: a headless app that shapes UI text with Inter and lays the
//! UI out the way Bevy 0.19.1's `UiPlugin` and `TextPlugin` do, without a
//! window, a renderer or picking.
//!
//! [`layout_app`] adds, in `PostUpdate`, the systems `UiPlugin` orders for
//! layout (`bevy_ui` `lib.rs`): `propagate_ui_target_cameras` in
//! `UiSystems::Prepare`, the two `HierarchyPropagatePlugin`s in
//! `UiSystems::Propagate`, `measure_text_system` in `UiSystems::Content`
//! (after `bevy_text`'s `load_font_assets_into_font_collection`) and
//! `ui_layout_system` in `UiSystems::Layout`. The UI camera is a `Camera`
//! with `IsDefaultUiCamera` whose viewport is the window's physical size,
//! which is what `propagate_ui_target_cameras` reads
//! (`Camera::physical_viewport_size`); the scale factor is 1, so logical
//! and physical px agree. [`crate::fonts::FontsPlugin`] installs the
//! repository's Inter in `Startup`, so text nodes are measured with the
//! font the game draws and with their own `TextFont` (size and features).
//!
//! Callers add their `Startup` spawn systems, then call `update` a few
//! times ([`settle`]) and read `ComputedNode` and `UiGlobalTransform`
//! through [`rect`].

use std::path::Path;

use bevy::app::{HierarchyPropagatePlugin, PropagateSet};
use bevy::asset::AssetPlugin;
use bevy::camera::Viewport;
use bevy::prelude::*;
use bevy::text::{FontCx, LayoutCx, RemSize, TextPipeline, load_font_assets_into_font_collection};
use bevy::ui::ui_surface::UiSurface;
use bevy::ui::update::propagate_ui_target_cameras;
use bevy::ui::widget::measure_text_system;
use bevy::ui::{
    ComputedUiRenderTargetInfo, ComputedUiTargetCamera, UiScale, UiSystems, ui_layout_system,
};

use crate::fonts::FontsPlugin;

/// An app that lays out UI nodes in a `window` of logical px at scale 1
/// with Inter installed as the default font. `Assets<Image>` exists (the
/// minimap's image node needs it).
pub(crate) fn layout_app(window: UVec2) -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        HierarchyPropagatePlugin::<ComputedUiTargetCamera>::new(PostUpdate),
        HierarchyPropagatePlugin::<ComputedUiRenderTargetInfo>::new(PostUpdate),
        FontsPlugin {
            assets: Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets"),
        },
    ))
    .init_asset::<Font>()
    .init_asset::<Image>()
    .init_resource::<FontCx>()
    .init_resource::<LayoutCx>()
    .init_resource::<TextPipeline>()
    .init_resource::<RemSize>()
    .init_resource::<UiSurface>()
    .init_resource::<UiScale>()
    .configure_sets(
        PostUpdate,
        (
            UiSystems::Prepare,
            UiSystems::Propagate,
            UiSystems::Content,
            UiSystems::Layout,
        )
            .chain(),
    )
    .configure_sets(
        PostUpdate,
        PropagateSet::<ComputedUiTargetCamera>::default().in_set(UiSystems::Propagate),
    )
    .configure_sets(
        PostUpdate,
        PropagateSet::<ComputedUiRenderTargetInfo>::default().in_set(UiSystems::Propagate),
    )
    .add_systems(
        PostUpdate,
        (
            propagate_ui_target_cameras.in_set(UiSystems::Prepare),
            load_font_assets_into_font_collection.before(UiSystems::Content),
            measure_text_system.in_set(UiSystems::Content),
            ui_layout_system.in_set(UiSystems::Layout),
        ),
    );
    app.world_mut().spawn((
        Camera {
            viewport: Some(Viewport {
                physical_size: window,
                ..default()
            }),
            ..default()
        },
        IsDefaultUiCamera,
    ));
    app
}

/// Run enough updates for spawned nodes to be propagated, their text
/// measured (a text node retries until its font is registered) and laid
/// out.
pub(crate) fn settle(app: &mut App) {
    for _ in 0..3 {
        app.update();
    }
}

/// The laid-out rect of `entity` in logical window px (top-left origin),
/// from its `ComputedNode` size and `UiGlobalTransform` centre.
pub(crate) fn rect(app: &App, entity: Entity) -> Rect {
    let node = app
        .world()
        .get::<ComputedNode>(entity)
        .expect("a laid-out node");
    let transform = app
        .world()
        .get::<UiGlobalTransform>(entity)
        .expect("a laid-out node");
    let centre = crate::hud::node_centre_logical(node, transform);
    Rect::from_center_size(centre, node.size() * node.inverse_scale_factor())
}

/// The one entity with component `C`.
pub(crate) fn single<C: Component>(app: &mut App) -> Entity {
    app.world_mut()
        .query_filtered::<Entity, With<C>>()
        .single(app.world())
        .expect("exactly one")
}
