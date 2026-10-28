# Art style

This document is the visual contract for Eonmark. It fixes the palette, material and lighting settings, the readability rule, the HUD look, the primitives-first approach and the rule for swapping in real models, the glTF import rules, the allowed asset sources and licences, and the screenshot checklist that every art PR is judged against. The look is calm, matte, flat-shaded low-poly 3D on a flat ground plane. The contract applies from M0 (the ground plane) through M7 (the look pass); the hex values are starting suggestions and are finalized at the M7 screenshot sign-off. Licensing rules are in [adr/0001-license.md](adr/0001-license.md).

## The look in one paragraph

A board game seen from a fixed-pitch camera. Flat colours, soft shadows, no shine, no glow. Units and buildings are simple silhouettes in team colour. Owned land is tinted a little and bounded by a soft line. The HUD is numbers and borders on matte panels. Nothing flashes, pulses or blooms. Readability at the far zoom beats detail at the near zoom.

## Palette

At most 12 base hues plus 2 team colours. Keep saturation low and value in the middle; nothing pure white, nothing pure black. These are suggestions in a matte range; the final values are measured at M7 and recorded here.

| Role | Suggested hex | Notes |
|---|---|---|
| Grass | `#7A9B5C` | Ground plane default |
| Forest floor | `#5C7A4A` | Forest tiles; tree clusters sit on it |
| Rock | `#8C8578` | Mountain tiles |
| Water | `#5B7C8F` | The impassable strip; flat, no animation |
| Soil and paths | `#A08C6A` | Build footprints |
| Timber | `#8B6B4A` | Lumber Yard, Supply Wain |
| Stone | `#B5AFA3` | Towns, Watchtower, Engine Yard |
| Thatch | `#B08D57` | Croft roofs |
| Ore | `#6E6A72` | Ore Pit, Mangonel frame |
| Grain gold | `#C9A961` | Grain resource icon and node |
| Lore ink | `#4F5B7A` | Scriptorium accent, Lore icon |
| Neutral and fog | `#3A3A3F` | Neutral territory, fog overlay colour, panel base |

| Team | Suggested hex | Notes |
|---|---|---|
| Blue | `#3F6FA8` | Player 1 default; match to the KayKit blue atlas swatch at M7 |
| Red | `#B04A3F` | Player 2 default; match to the KayKit red atlas swatch at M7 |

HUD accents are drawn from the same table plus two states:

| HUD role | Suggested hex |
|---|---|
| Text | `#ECE9E2` |
| Panel | `#26262A` at 92% opacity |
| Disabled | `#6B6B70` |
| Yield Cap amber | `#D29B3B` |
| Affordable or valid placement | `#6FA06A` |
| Unaffordable or invalid placement | `#B04A3F` |

## Material settings

One `StandardMaterial` per kind and team, shared by every instance of that kind.

| Property | Value |
|---|---|
| `perceptual_roughness` | 0.95 |
| `reflectance` | 0.2 |
| `metallic` | 0.0 |
| `emissive` | black; never used for gameplay signalling |
| Alpha | opaque, except HUD panels; territory is drawn inside the opaque ground material by `FieldExt`, not as a separate overlay |

## Lighting and post-processing

| Setting | Value |
|---|---|
| Lights | One `DirectionalLight` with soft cascaded shadows, plus `AmbientLight` |
| Tonemapping | `Tonemapping::TonyMcMapface` |
| Anti-aliasing | `Msaa::Sample4` |
| Bloom | none |
| Emissive glow | none |
| SSAO | optional, low strength only; off by default |
| Other post-processing | none |
| Present mode | `PresentMode::AutoVsync` (Mailbox is unsupported on Metal) |

## Readability at 40 m

The camera zooms between 15 m and 60 m at a pitch of about 55 degrees. The acceptance distance is 40 m: at that zoom, in a 1280x720 screenshot, every unit kind must be identifiable by silhouette and every team by colour. Shape carries identity, colour carries ownership. If a kind is only distinguishable up close, change its silhouette, not its texture.

Primitive silhouettes (M2 to M7):

| Kind | Primitive | Notes |
|---|---|---|
| Units | KayKit Adventurers characters, the KayKit catapult and a loaded wheelbarrow since art slice 4 (the table under "Unit looks (art slice 4)"); each kind's M4b look (table below) is its fallback | Team colour on clothes, plate and banners; M4b gave each kind its own primitive silhouette, M2 drew every unit as a smooth `Capsule` |
| Buildings | KayKit Hexagon models since art slice 3 (the table under "Building looks"); `Primitive(Cuboid)`, the M3b team-coloured box over the footprint, is every building's fallback | Team colour on roofs and trim, plus a team banner beside each building |
| Town | KayKit castle; a Seat flies the Seat flag on its keep | The Seat flag (1.8 m) flies on top of the keep, wholly above the Town's own banner (a `check` rule), so a Seat reads apart from any other Town by its place, not its size: other banners are larger (the Town's own 2.83 m, the Muster Hall's 2.16 m, the Mangonel's 1.94 m) |
| Construction site | KayKit construction stages A, B and C, one per third of the build, neutral | Scaled by the footprint side; the health bar shows the progress |
| Rubble | KayKit `building_destroyed`, neutral, for 30 s | Where a building or site was destroyed; gone early when a new building takes the footprint |
| Terrain props | KayKit trees on forest, mountains and rocks on mountain, lilies and plants on water (`terrain` in `visuals.ron`) | Never on grass; a look only where its footprint stays on its own terrain; nothing over 1.6 m on a tile whose north neighbour is passable, so props never hide units |
| Selection ring | Flat annulus mesh, unlit (retained gizmo lines until the art pass) | Separate entities keyed by id, never children of moving units; one shared mesh and material for every ring, so any number of rings is one draw call |
| Rally marker, move marker | Retained gizmo lines | Same rule |
| Attrition indicator (M5a) | Flat 4-segment annulus mesh, unlit (retained gizmo lines until the art pass) | Same rule; a steady Yield Cap amber (#D29B3B) diamond under a unit bleeding on enemy land, one shared mesh and material for every diamond; nothing pulses |
| Supply ring (M5a) | Retained gizmo lines | Same rule; a Timber ring around a selected Supply Wain; nothing pulses |
| Annexation timer (M5a) | Immediate gizmo lines, 3 px | A #3A3A3F track circle just outside a contested Town and the attacker's team-colour arc filling clockwise from north |
| Health bar | Flat quad entity | Same rule; unlit, a team-colour fill on a #3A3A3F track, no green-to-red, shown only while the target is damaged or selected (M4b, `health_bars.rs`) |

Unit looks (M4b, `data/visuals.ron`; since art slice 4 each kind's `fallback`, drawn in headless runs and when its model fails), one silhouette per kind. Width is the kind's collision diameter from `units.ron`; each mesh has 10 facets around (a 6-latitude capsule, an icosphere of 80 faces) and flat face normals, at most 600 vertices, one mesh per kind and one material per kind and team:

| Kind | Look | Silhouette |
|---|---|---|
| Yeoman | `Primitive(Capsule)` | Upright pill, 0.7 m wide, 1.19 m tall |
| Scribe | `Primitive(Cylinder)` | Drum, 0.7 x 0.7 m |
| Skirmisher | `Body(shape: Cone, height_tiles_x100: 150)` | Spike, 1.5 m |
| Shieldbearer | `Body(shape: Cuboid, height_tiles_x100: 130)` | Block, 0.8 x 1.3 x 0.8 m |
| Bowman | `Body(shape: Cylinder, height_tiles_x100: 190)` | Tall post, 1.9 m |
| Outrider | `Body(shape: Capsule, height_tiles_x100: 180, lying: true)` | Pill lying along its facing, 1.8 m long (twice its collision diameter; see the crowd check below), 0.9 m tall |
| Mangonel | `Primitive(Sphere)` | Ball, 1.0 m |
| Supply Wain | `Body(shape: Cuboid, height_tiles_x100: 60)` | Low box, 1.0 x 0.6 x 1.0 m |

Two kinds share a shape only when they differ by at least 1.4 times on some axis (the Scribe's drum and the Bowman's post, the Shieldbearer's block and the Wain's low box, the Yeoman's upright pill and the Outrider's lying one); `present::tests::every_unit_kind_has_its_own_silhouette` checks it. The 40 m view is `--scenario unit_looks`: one unit of each kind per team in two rows 15 m apart, framed at 40 m unless `--camera` is given (`target/ci/eonmark --background --scenario unit_looks --exit-after-seconds 40 --screenshot-at 100:looks.png`; CI's render smoke test renders it on every PR). The owner chose single-shape silhouettes over multi-part looks on 2026-10-22 (ROADMAP M4b, Notes, decision 11); multi-part looks wait for the art pass. The looks themselves are signed off on the 40 m screenshot, and that sign-off is still pending: the owner's run on the Mac, with the P3 and sRGB check and the checklist below.

The sign-off also looks at a crowd, because the lineup stands every unit alone 4 m from the next and cannot show how the looks pack in a fight: `target/ci/eonmark --background --scenario enemy_outpost --camera 100,32,40 --exit-after-seconds 150 --screenshot-at 250:crowd.png` (`just render-smoke out enemy_outpost 100,32,40 250 150` under Xvfb; with unit models lavapipe reaches tick 250 about 93 s in) frames the 200 vs 200 prologue at 40 m mid-fight. Every look is exactly its kind's collision diameter across except the Outrider's, whose lying pill is 1.8 m long against a 0.9 m diameter. Melee range is measured edge to edge (0.25 m for the Outrider) and the sim keeps units apart only by a soft push, so two Outriders holding on each other stand at most 1.15 m apart, centre to centre, while each pill reaches 0.9 m ahead of its centre: head-on they overlap by at least 0.65 m, and in a packed melee the pills visibly pass through each other. Shortening the pill is a data-only change, `height_tiles_x100` of `"outrider"` in `data/visuals.ron` (120 keeps a lying pill, cuts the head-on overlap to 0.05 m and still differs from the Yeoman's upright pill by 1.7 times along its length, and `data-check`'s 1.4x silhouette check (`rules::Visuals::validate`; at M4b it was `present::tests::every_unit_kind_has_its_own_silhouette`, which since the art pass measures the frozen M4b looks) passes with it, but reads closer to the Mangonel's 1.0 m ball); since `rules_version` 25 `visuals.ron` is not in `rules_hash` ([ADR 0003](adr/0003-visuals-outside-rules-hash.md)), so it needs no `rules_version` bump and no `just rerecord` (at M4b it needed both). The owner decides that trade at the sign-off.

Motion is procedural, not animation clips, in v0.1: since art slice 4 walking units (civilians, infantry, cavalry) bob up to 5 cm once per 0.6 m walked (`present::Gait`, presentation only), while siege engines and the Wain roll; hit feedback follows in slice 5.

## Territory and fog

| Element | Value |
|---|---|
| Owned-land tint | about 12% team colour over the ground |
| Border | a 1-tile soft isoline where ownership changes |
| Neutral or tied tiles | no tint, except within half a tile of owned land, where the owner's tint fades from about half of the 12% at the edge to nothing at the tile centre, under the border line |
| Fog, explored | ground and buildings darkened to 0.45 |
| Fog, unexplored | 0.0 (hidden) |
| Supply coverage | a thin Timber (#8B6B4A) ring of radius 14.5 m around a selected Supply Wain (M5a, `markers.rs`); the subtle hatching from the same field texture's A channel waits for M7 with fog (M5a owner decision 10) |

The ground's material is the `FieldExt` extended material (`crates/game/src/field.rs`, shader `field.wgsl` embedded in the binary) reading an RGBA8 field texture with one texel per tile, 128x128 on the current maps (R owner, G strength, B fog, A supply). Each owner's membership is interpolated between tile centres with smoothstep weights; the tint follows it and the border is the isoline where it crosses one half, fading to nothing at the tile centres on either side, so the line follows the buildable tiles with rounded corners. M3b drew a vertex-coloured version first and replaced it within the milestone. The shader is mandatory by M7 because fog rides the same texture.

## HUD

`bevy_ui` only in shipped builds. Inter (variable weight) for all text. Kenney UI Pack 9-slice panels through `NodeImageMode::Sliced`. Matte panels, thin borders, no drop shadows, no glow. The top resource bar shows current stockpile and per-30-s rate per resource and turns the readout amber while production is being discarded at the Yield Cap. Disabled command-card buttons are greyed with a tooltip, not hidden. Layout-only root nodes are `Pickable::IGNORE` so empty HUD space passes clicks to the world. `bevy_egui` appears only behind the `dev` feature.

## P3 and sRGB check

wgpu on a wide-gamut display can render colours more saturated than authored. Every palette or art change is judged twice: once on the development Mac's P3 display, once as an sRGB screenshot opened in Preview with the display profile set to sRGB. Both must look matte. If a colour reads as neon in either, lower its saturation until both agree.

## Primitives first, models by data

Every unit kind is drawn through one `Visual` value from `data/visuals.ron`, keyed by its `units.ron` id, and since the art pass every building kind too, keyed by its `buildings.ron` id (`Primitive(Cuboid)` is the M3b box `crates/game/src/buildings.rs` draws; slice 1 mapped all eight kinds to it, slice 3 maps them to KayKit models, "Building looks" below):

```ron
"yeoman": Primitive(Capsule),
"skirmisher": Body(shape: Cone, height_tiles_x100: 150),
"outrider": Body(shape: Capsule, height_tiles_x100: 180, lying: true),
"mangonel": Scene(path: "models/mangonel.glb", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0, fallback: Some(Primitive(Sphere))),
```

`Visual::Primitive(shape)` draws a team-coloured primitive sized from the kind's radius (`Capsule`, `Cuboid`, `Cylinder`, `Sphere` or `Cone`). `Visual::Body(shape, height_tiles_x100, lying)` (M4b) draws the same primitive two radii wide with its own height in hundredths of a metre (20 to 400; never a `Sphere`); `lying: true` lays a `Capsule` or `Cylinder` along the unit's facing, that long and two radii tall. `Visual::Scene(path, scale_x100, y_offset_tiles_x100, yaw_deg)` loads a glTF scene; the scale and offset are in hundredths and the yaw in whole degrees, so the file stays float-free. Swapping a cube for a model is a RON edit, never a Rust change, and since `rules_version` 25 it needs no `rules_version` bump or re-record either: `data-check` validates `visuals.ron`, but `rules_hash` leaves it out ([ADR 0003](adr/0003-visuals-outside-rules-hash.md)). M0 to M5b shipped primitives only; the art pass (before M6, pulling M7's look items forward) maps every kind to CC0 models and keeps each M4b look as its fallback. Anything drawn above a unit (the M4b health bar) takes its height from `present::UnitVisuals::top_y`: the model's top (or the top of the team banner the kind carries, when that is higher, so the Mangonel's bar rides over its banner) when the model is drawn, else `present::look_top_y` of the fallback look. Since art slice 3, `crates/game/src/models.rs` loads every Scene the game draws as a model once, merges each into one placed mesh, and holds the sim's first tick until every model is ready or has failed (the load gate in `sim_driver::match_running`), so nothing appears as a box and then swaps; a building whose model fails draws the box with one `error!` (the render smoke fails on it). Since art slice 4 a unit `Scene` draws its merged model too (`present::UnitVisuals::mesh_with`), one mesh per kind and one material per texture and team, and its `fallback` (else `Primitive(Capsule)`, `present::FALLBACK_VISUAL`) in headless runs and when its model fails; bars over a unit take the model's top (or its banner's). Bars and attack markers over a building take its top from `buildings::building_top_y` (the model's top, a site's stage times its scale, or the box). `data-check` holds the 1.4x silhouette rule over the live file's `Primitive` and `Body` looks (a Scene by its fallback), while the `present.rs` look tests measure the M4b looks frozen as `M4B_LOOKS`, so a look edit never fails a Rust test.

## glTF import rules

These replace the Blender-export rules of M0; they describe the files the art pass vendors and bakes, and `inspect_gltf check` enforces every row it can measure.

| Rule | Value |
|---|---|
| Format | glTF 2.0: a pack's `.gltf` + `.bin` + one PNG as shipped, or a baked `.glb` (art slice 4) |
| Up and units | +Y up, 1 unit = 1 m; any other size is set by `scale_x100`; `convert_coordinates` stays off |
| Facing | units face +X at rest after `yaw_deg`; buildings, sites and rubble put their front toward +Z, toward the camera; props have no facing. KayKit Hexagon buildings already face +Z at yaw 0 (their doors, the mine's entrance and the forge face +Z; screenshots at four yaws, art slice 3), so every building uses `yaw_deg: 0`. The KayKit flag's cloth faces ±X, so the banner and the Seat flag turn it with `yaw_deg: -90` to face the camera |
| Origin | not required in the file. The placed model's bounds must have their base within -0.02..0.05 m and their x/z centre within 0.10 m of the origin (0.5 m for props), set with `y_offset_tiles_x100`, `x_offset_tiles_x100` and `z_offset_tiles_x100` (art slice 3), applied after the yaw and the scale |
| Contents | indexed triangles with `POSITION`, `NORMAL` and `TEXCOORD_0` on every kept primitive; one base-colour texture per model; no cameras or lights; skins and animations only in bake inputs |
| Material | the file's material gives only its base colour and texture; the game builds `StandardMaterial { perceptual_roughness: 0.95, reflectance: 0.2, metallic: 0 }`, no emissive |
| Sampler | nearest, forced with `override_sampler` |
| Texture | one palette or gradient texture at most 256 px on a side, except a pack's shared palette atlas, which ships as is (KayKit Hexagon's 1024 px `hexagons_medieval.png`, art-pass owner decision 5); baked characters are downsampled only when every face keeps its colour |
| Naming | the pack's own names under `models/<pack>/...`; baked files `models/kaykit_adventurers/<kind>.glb` |
| Inspection | `cargo run -p inspect_gltf --release -- inspect <file>` prints scenes, the node tree with transforms, meshes and primitives, images, skins, clips, rest bounds and a T-pose hint; `swatches`, `palette`, `bake` and `check` are described below |

Node globs in a Scene's `nodes` match a node's own name only (`rules::visuals::glob_match`, shared by the game's merge and the tool), so keeping a node keeps its mesh, not its children.

## Building looks (art slice 3)

Each building kind is one line of `data/visuals.ron`: a KayKit Hexagon model (blue variant, recoloured at run time), scaled to fill 0.95 of its footprint side (`inspect_gltf check` prints the suggestion), lifted 1 cm, centred where the file is off-centre, at yaw 0, with the owner's `kaykit_hexagon` swatch and a team banner on it, inside its footprint. Team shares are measured by `inspect_gltf check` on the model plus its banner at its data scale (owner decision 12 of the art pass, 2026-10-28):

| Kind | Model | Scale | Offsets (cm) | Placed size (m) | Banner (height) | Team share: model / with banner |
|---|---|---|---|---|---|---|
| Town | `building_castle_blue` | 126 | - | 2.49 x 5.01 x 2.84 | 2.83 m, its pole in the front-left tower, the cloth over the gate (top 3.74 m); the Seat flag 1.80 m on the keep (foot 4.90 m) | 67‰ / 102‰ |
| Croft | `building_home_A_blue` | 223 | - | 1.77 x 2.07 x 1.90 | 0.69 m on the front of the ridge | 258‰ / 263‰ |
| Lumber Yard | `building_lumbermill_blue` | 139 | z -19 | 1.90 x 2.37 x 1.65 | 1.53 m, from the mill's front over the log yard | 64‰ / 102‰ |
| Ore Pit | `building_mine_blue` | 99 | - | 1.59 x 1.13 x 1.90 | 2.33 m at the mine's front-left foot, turned 45° so it fits, the cloth over the mine | 5‰ / 102‰ |
| Scriptorium | `building_church_blue` | 247 | - | 2.54 x 4.06 x 2.85 | 0.83 m in the free front-left corner | 280‰ / 283‰ |
| Muster Hall | `building_barracks_blue` | 182 | z -12 | 2.62 x 2.98 x 2.85 | 2.16 m, from the west wing over the roof (top 3.40 m) | 84‰ / 102‰ |
| Watchtower | `building_tower_A_blue` | 82 | - | 0.81 x 1.80 x 0.95 | 0.83 m on the front edge of its roof | 81‰ / 109‰ |
| Engine Yard | `building_blacksmith_blue` | 221 | x -3, z -14 | 2.85 x 2.18 x 2.75 | 0.97 m in the free front-left corner | 91‰ / 102‰ |

The banner is the KayKit flag (48 triangles, 457‰ team area), one shared mesh with one material per team (the same team material as the buildings on the atlas), so all banners add about two draw bins; banners and the Seat flag cast no shadow. Its `banner_at` mount places the flag's centre from the footprint centre on the ground, its pole stands 0.11 m x scale to the left of that point, and its yaw turns it (the cloth is two-sided). Every building reaches the 100‰ floor with its banner (owner decision 12 keeps the floors), and `check` holds four rules on the mounts. A banner and the Seat flag stay inside the footprint in plan view, so no cloth hangs over a neighbour's tiles, into the forest or mountain beside a Lumber Yard or Ore Pit, or over the units walking there (review of slices 3 and 4, 2026-10-28: the first banners stood beside their buildings and hung up to 1.81 m past them). The models fill 0.95 of the footprint, so a banner either stands in a free corner of the footprint (the Scriptorium, the Engine Yard) or on the building, its pole sunk into the roof so the cloth clears it. The Seat flag's foot stands above the top of the Town's banner. A banner taller than its building is printed as a note, not a failure: the Ore Pit's 2.33 m banner on a 1.13 m mine and the Town's 2.83 m banner over its gate are the sizes the 100‰ floor takes on 5‰ and 67‰ of model team colour (the Ore Pit's must also turn 45° to fit its 2 m footprint), and whether they read well at 40 m is the owner's call at the sign-off; a smaller banner there would miss the floor. (Slice 3 had capped a banner at its building's height and the Seat flag at 1.4 times the Town's banner, and held the Town and the Ore Pit under the floor for the owner; those were the implementer's rules, not decision 12's, and the hold let CI pass two buildings under a floor the owner kept, so both rules went in the review.) A site carries no banner (the stages are neutral scaffolding; its builders show whose it is), rubble never does, and neither does the M3b box a building falls back to when its model fails, which is team-coloured all over; a Seat drawn as that box flies its flag on the ground at the footprint's north-east corner.

Sites draw the KayKit construction stages A, B and C (neutral, scaled 91, 84 and 82 so one tile of footprint is 0.95 m, then scaled by the footprint side), rubble `building_destroyed` (scale 62, 30 s). Every neutral model (props, the stages, rubble) is desaturated 50% toward grey (`neutral_desaturate_permille: 500`, review of 2026-10-28), as a team texture desaturates the texels outside its swatch: as shipped, KayKit's emerald trees had a median HSV saturation of 0.77 in the 40 m Town shot, twice the team roofs' 0.34-0.36 and the strongest hue on screen; at 500 they read 0.27, beside the grass's 0.28, and the pixels at saturation 0.5 or more in that shot went from 17,185 green, 12,222 red and 8,160 blue to 10, 10,291 and 10,651. Terrain props: forest interiors take the four KayKit tree patches at 130% (1.2-1.7 m), forest edges the single trees (1.2-1.3 m); mountain interiors the three mountains at 110% (1.6-2.0 m) and, two rings deep in a range, mountains B and C at 190% (2.9-3.4 m); mountain edges single rocks; water 6% lilies and plants, without shadows.

### Swapping a building's look (walkthrough)

Swapping a look is one line of `data/visuals.ron` and no Rust. Here the Croft goes back to the M3b box:

```bash
cp data/visuals.ron /tmp/visuals.ron.bak
sed -i 's|^    "croft": Scene(.*|    "croft": Primitive(Cuboid),|' data/visuals.ron
cargo run -q -p sim-cli -- data-check data      # OK ... visuals=units:8 buildings:8 terrain:3 models=<n>
cargo nextest run -p game --cargo-profile ci    # green: no test reads the live building looks
git diff --stat                                 # data/visuals.ron only
just verify                                     # every fixture OK: visuals.ron is not in rules_hash
cp /tmp/visuals.ron.bak data/visuals.ron
```

The Croft's line carries its banner mount, so the box (which is team-coloured all over) has none. The swap back to a model is the same edit the other way; `inspect_gltf check` then prints the new model's measurements. The literal walkthrough of the ROADMAP's M7 line, the Yeoman back to `Primitive(Capsule)`, is under "Unit looks (art slice 4)" below.

## Unit looks (art slice 4)

Every unit kind is one line of `data/visuals.ron`: a static model with its team swatch and its M4b look as the `fallback`. Six are KayKit Adventurers 1.0 characters (CC0), posed and baked by `just art-bake` from `tools/assets/bake.ron`: `inspect_gltf bake` samples one moment of a clip (all 76 clips of every character are LINEAR; none is CUBICSPLINE), skins the mesh, keeps the named nodes, merges them, puts the base centre at the origin facing +X, halves the 1024 px texture twice to 256 px (every face keeps its colour within 8/255), and writes a GLB that records its Modifications text; each output is pinned by sha256 in its `bake.ron` row, which `scripts/check_assets.sh` holds it to, and baking twice is byte for byte the same. Every character rests in a T-pose in its file (rest arm-span ratio 0.89 on the Mage to 1.35 on the Rogue), so a pose is what makes it read. Measured by `inspect_gltf check` (heights and sizes as placed, x by y by z, +X ahead):

| Kind | Source, kept nodes | Pose | Scale | Placed size (m) | Triangles | Team swatch | Team share |
|---|---|---|---|---|---|---|---|
| Yeoman | `Rogue_Hooded.glb`: body, hooded head, cape | `Idle` | 58 | 0.67 x 1.30 x 0.77 | 4,005 | `adv_rogue`: green hood, cloak and sleeves | 668‰ |
| Scribe | `Mage.glb`: body, hat, cape, `Spellbook_open` | `Spellcasting` (the book held out) | 53 | 1.07 x 1.41 x 1.19 | 4,801 | `adv_mage`: purple robe and hat, magenta cape and book cover | 596‰ |
| Skirmisher | `Barbarian.glb`: body, fur hat, cape, `1H_Axe`, `1H_Axe_Offhand` (the round shield dropped) | `2H_Melee_Idle` (axes crossed in front) | 59 | 1.19 x 1.39 x 0.79 | 5,325 | `adv_barbarian`: blue cloth, fur hat and cape, axe hafts | 446‰ |
| Shieldbearer | `Knight.glb`: body, helmet, cape, `Rectangle_Shield`, `1H_Sword` | `Blocking` (tower shield in front) | 60 | 1.19 x 1.44 x 0.90 | 5,274 | `adv_knight`: red tabard and cape, steel plate, two-handed grip | 471‰ |
| Bowman | `Rogue_Hooded.glb`: body, hooded head, cape, `2H_Crossbow` | `2H_Ranged_Aiming` (crossbow levelled) | 63 | 1.08 x 1.41 x 1.15 | 4,797 | `adv_rogue`: green hood, cloak and sleeves | 589‰ |
| Outrider | `Knight.glb`: body, helmet, cape, `2H_Sword` | `2H_Melee_Idle` | 84 | 1.68 x 2.04 x 1.53 | 5,124 | `adv_knight` | 523‰ |
| Mangonel | `building_tower_catapult_blue`: the `catapult_*` nodes (turret and arm; the tower dropped), turned 90° to face +X and lowered 1.95 m from the tower top | static | 140 | 1.45 x 0.79 x 0.99 | 1,326 | `kaykit_hexagon`, plus the team banner at scale 700 (1.94 m) | 25‰ / 204‰ with the banner |
| Supply Wain | `wheelbarrow` with `sack`, `crate_A_big` and `flag_blue` (scale 140) as `parts`, turned 90° (wheel ahead) | static | 220 | 1.57 x 1.34 x 0.52 | 668 | `kaykit_hexagon`: the flag as a pennant | 88‰ |

The Bowman is the hooded Rogue since the review of 2026-10-28: baked from the bare `Rogue.glb`, its brown hair (hue about 17°, beside the red team's 6°) covered most of what the 55° camera sees, so at 40 m a blue Bowman showed 9-12% team pixels against 46-87% on the other kinds and read as part of the red army, although `check` measured 282‰ of its whole surface; the hood is in the `adv_rogue` swatch, so it shows 589‰ and the team colour now covers the head. The crossbow still keeps it 1.61 times deeper than the Yeoman.

The heights are the art-pass plan's: the Yeoman 1.30 m, the Scribe, Skirmisher and Bowman 1.40 m, the Shieldbearer 1.45 m and the Outrider 2.05 m, 1.41 times the Shieldbearer (no CC0 horse is reachable, so it is the Knight with a two-handed sword, drawn larger). Each character's swatch rects are cells of its baked 256 px atlas picked with `inspect_gltf palette` (the Adventurers ship in one colour, so `swatches` has no red variant to diff); the Knights' red tabard alone is 55-61‰ of their surface, so their swatch takes the steel plate too, and the Barbarian's blue cloth (70‰) takes the fur hat and cape. The rest of each atlas is desaturated 25%, so team colour stays the strongest hue. The Mangonel's catapult carries 25‰ of team texels, so it carries the team banner (owner decision 12 of the art pass): one shared mesh with every building's banner, a top-level entity that follows the unit (`present::UnitBanner`), measured with the model. A banner of a readable 1.25 m (scale 450) gives 123‰; the shipped 1.94 m one meets the 200‰ floor, and its KayKit pole is 0.39 m thick at that scale. A unit's banner may stand taller than the unit (a standard over a siege engine), and unlike a building's it is not held inside a footprint, since it moves with its unit. At scale 700 it reaches 1.32 m behind the Mangonel's centre and 0.6 m to its right, well past its 0.5 m collision radius, and the sim keeps units apart only by a soft push (about 1.0-1.1 m centre to centre in a packed group), so in a group of Mangonels, or a crowd, the poles and cloths pass through the neighbouring catapults and units (`art_bench`'s ten engines at tick 60 make a fence of crossing planks). A smaller banner clips less but misses the 200‰ floor: scale 450 at offset (-20, 0, 0) still reaches 0.79 m behind and gives 123‰, and staying inside the radius takes about scale 380, centred, which gives 100‰. The owner decides that trade at the sign-off, as for the Outrider's overlap above. The Mangonel's health bar rides 0.3 m over the banner's top (2.24 m), not over the catapult's, so the cloth never covers it. The Wain meets its 80‰ floor with the flag scaled 1.4 times as a pennant on its crate.

The triangle budget is the art-pass plan's unit cap of 5,500 (slice 2 measured 4,005-5,325 with the kept items; no lighter CC0 character is reachable). `art_bench` (400 units, two full Towns, 60 m) draws `triangles main_max=2117469` in the main pass under lavapipe, under twice the plan's 1.2 M estimate for units, so no cape, helmet or hat was dropped; the owner's Mac run (AP5) decides whether any must go. Draw calls do not grow: each kind is one mesh, the characters share one material per atlas and team, the Mangonel and the Wain the Hexagon one, and the banners the building banners' bin (`art_bench` max 105, as in slice 3 with the M4b looks).

**The 1.4x rule per rig group.** Two unit kinds that look alike must differ by at least 1.4 times in width, height or depth. `data-check` holds it over the `Primitive` and `Body` looks (a Scene by its fallback), and `inspect_gltf check` over the placed model bounds within a rig group: units that share a rig (a baked file's recorded skeleton, else the file) and a base texture, that is, variants of one character. Every Adventurers character is skinned to the same 41-joint skeleton, so the texture tells them apart; the groups are the two Rogues (Yeoman and Bowman: 0.67 against 1.08 m deep, 1.61 times) and the two Knights (Shieldbearer and Outrider: 1.44 against 2.04 m tall, 1.41 times), while the Mage and the Barbarian stand alone, as do the Mangonel and the Wain. A new kind on an existing character must clear the rule against its sibling, by its pose (an item held out) or its height (`scale_x100`). The human check is the 40 m lineup (`--scenario unit_looks`).

Every unit is picked by ray-casting its merged model (rule 16). `--scenario unit_looks --click-check` prints a click target per unit at tick 60, the pixel nearest the centroid of its projected model that shows the unit, and judges real clicks against them (`crates/game/src/click_check.rs`); driven with xdotool under Xvfb on 2026-10-28 it printed `click_check: ok 16/16` with every target on its centroid, so no picking proxy (`pick: Capsule`) was needed. Aiming at the centre of a model's bounds instead missed the red Skirmisher once: that point falls in the gap between its body and the axes it holds out.

### Swapping a unit's look (walkthrough)

This is ROADMAP M7's line "swapping one entry back to `Primitive(Capsule)` requires no Rust change", done literally: one line of `data/visuals.ron`, no Rust, no `rules_version` bump and no re-record.

```bash
cp data/visuals.ron /tmp/visuals.ron.bak
sed -i 's|^    "yeoman": Scene(.*|    "yeoman": Primitive(Capsule),|' data/visuals.ron
cargo run -q -p sim-cli -- data-check data      # OK ... rules_hash=0xfb0d0b8986fc8865 ... models=35
cargo nextest run --workspace --cargo-profile ci --profile ci   # green: no test reads the live unit looks
git diff --stat                                 # data/visuals.ron only
just verify                                     # every fixture OK: visuals.ron is not in rules_hash
cp /tmp/visuals.ron.bak data/visuals.ron
```

The Yeoman then draws the M4b capsule in its team colour (its model file is simply no longer loaded), and `data-check`'s 1.4x rule still holds, since the capsule is the Yeoman's fallback anyway. Swapping it back is the same edit the other way. Run on 2026-10-28 (`cargo test --workspace --profile ci` standing in for nextest, which that VM lacks): `models=35` against 36, every test green, `git diff --stat` `data/visuals.ron | 2 +-` alone, all 11 fixtures OK, and `unit_looks` drew the blue and red capsules at the head of the rows.

## Allowed asset sources

| Source | Licence | Use |
|---|---|---|
| Kenney (UI Pack, Game Icons, Nature Kit, and other kits) | CC0-1.0 | UI 9-slice, icons, trees, props |
| KayKit (Medieval Builder, Medieval Hexagon, Adventurers) | CC0-1.0 | Buildings with team colours, units |
| Freesound, filtered to CC0 | CC0-1.0 | About 10 OGG clips (M7) |
| Inter | OFL-1.1 | The one UI font; licence text vendored beside the `.ttf` |
| Any other source | CC0-1.0, CC-BY-4.0 or OFL-1.1 only | Each with an `assets/ATTRIBUTION.md` row |

Excluded: Quaternius (proprietary licence since 2026-08-28), CC-BY-SA, CC-BY-NC, GPL art, Sampling+, and any material from the inspiration's fan wiki.

Each vendored file gets an `ATTRIBUTION.md` row: asset, author, source URL, licence, modifications, download date. In-house files are marked `original: yes` and are CC0. `scripts/check_assets.sh` fails CI if a non-allowlisted file lacks a row, exceeds 2 MB, or the total exceeds 300 MB, and `scripts/check_assets.sh --self-test` proves that an unattributed file fails with its path. No Git LFS.

### Where the art pass's files come from

The pre-stream art pass ([ROADMAP.md](ROADMAP.md), "Art pass (pre-stream)") vendors KayKit Medieval Hexagon 1.0 (buildings, construction stages, rubble, flag, cart props and nature) and Inter 4.1, and bakes its units from KayKit Adventurers 1.0. Everything is fetched from `raw.githubusercontent.com` (the `KayKit-Game-Assets` organisation, which the owner confirmed as the source to use, and `rsms/inter` at tag `v4.1`), the only host that serves these files to the agent's VM: github.com pages, its git and API hosts, kenney.nl, itch.io and opengameart.org refuse, and KayKit Medieval Builder and Forest Nature are itch-only (404 on raw). So:

- **Pins are sha256, not commits.** `tools/assets/sources.tsv` lists every remote file with its URL, sha256, licence, author and per-file GitHub page; `just fetch-assets` (`scripts/fetch_assets.sh`, maintainer-only, never in CI) fetches and checks the licence texts first (KayKit's `LICENSE.txt` must name `publicdomain/zero/1.0`, Inter's must be the SIL Open Font License 1.1, or it stops), then every other file into a download cache, verifies each pin and copies the vendored ones into `assets/` byte for byte. The vendored copies are the source of truth; a second run changes nothing. Offline, `scripts/check_assets.sh` (CI's check job and `just ci`) hashes every vendored file against its pin, so a file whose `ATTRIBUTION.md` row says Modifications `none` stays byte for byte as shipped: an edit fails CI naming the file until its row and its Modifications text record it. `.editorconfig` turns off whitespace trimming under `assets/`, since KayKit's licence texts end lines in tabs.
- **Tool-only inputs stay in the cache.** The red KayKit variants exist only for `inspect_gltf swatches`, and the 3.6 MB rigged character files only for `inspect_gltf bake` (`just art-bake`, `tools/assets/bake.ron`); neither is committed. The bakes (art slice 4: six files of 157-226 KB) are each pinned by the `sha256` of their `bake.ron` row, which `scripts/check_assets.sh` checks like a `sources.tsv` pin (its `--self-test` changes a byte of one to prove it), and attributed with the Modifications text the bake records in the file.
- **Licence texts are vendored with their own rows** (`models/kaykit_hexagon/LICENSE.txt`, `models/kaykit_adventurers/LICENSE.txt`, `fonts/OFL-Inter.txt`), and `inspect_gltf`'s `attribution_rows_match_the_sources` test holds `ATTRIBUTION.md` to `sources.tsv` and `bake.ron`.
- **Kenney is deferred to M7** (UI Pack panels, Game Icons): its site is unreachable from the agent's VM, so the owner fetches it on the Mac then. The KenneyNL starter kits on raw GitHub are not used: no per-asset CC0 statement has been read there.

### Budgets (measured in art slice 2)

`inspect_gltf check data assets` (CI's check job) holds every Scene in `data/visuals.ron` to these limits. The file, texture and triangle limits were set from `inspect_gltf inspect` on every chosen source, each a little above the heaviest file it covers. The team-area floors were not measured: they are the plan's (its section 3.4); since art slice 3 a building's share counts its banner (below the table). Each `ok` line also prints `facing=`, the horizontal side with the most surface and its share (a hint for the rest-facing rule: units face +X after `yaw_deg`, buildings, sites and rubble +Z toward the camera; props print `-`); a share near 25% means no side dominates. It does not find a door: on the KayKit buildings it reads 26-29% on whichever side is largest (for example `+X(29%)` for the Croft, whose door faces +Z), so the yaws were set from screenshots and the 40 m screenshots decide.

| Limit | Value | Heaviest measured |
|---|---|---|
| File | 2 MB | castle `.bin` 317,732 B |
| Texture side | 256 px, except the shipped Hexagon atlas | `hexagons_medieval.png` 1024 x 1024 (owner decision 5); the characters embed 1024 x 1024, baked to 256 x 256 (art slice 4) |
| Unit triangles | 5,500 (a note above 5,000) | Adventurers body plus kept items 4,005 (hooded Rogue) to 5,325 (Barbarian with two axes); Mangonel catapult nodes 1,326; Wain parts 668 |
| Building triangles | 6,000 | castle 5,659 (under the plan's 12,000, so it stays) |
| Site and rubble triangles | 1,500 | `building_stage_C` 1,218 |
| Seat flag and banner triangles | 100 | `flag_blue` 48 |
| Interior prop triangles | 1,600 | `trees_B_large` 1,540 (the large trees stay) |
| Edge prop triangles | 300 | `tree_single_B` 220 |
| Team area | units 200‰, the Wain 80‰, buildings 100‰ (model plus banner), the Seat flag and the banner 300‰ | Hexagon buildings 5‰ (mine) to 280‰ (church); flag 457‰ |

The team area is the share of a model's surface whose faces sample its `team_swatches` rects. KayKit Hexagon's team texels are one rect of the shared atlas, `(17, 787, 91, 207)`, found by `inspect_gltf swatches` over the blue and red variants of every vendored building, the catapult tower and the flag; no non-team face samples it, and no neutral model (stages, rubble, nature, cart props) does either.

**The chosen models against the team floors** (a trial `check` with the building mapping of the art-pass plan, every building `team: Some("kaykit_hexagon")`, slice 2):

| Use | Model | Team area | Floor |
|---|---|---|---|
| town | `building_castle_blue` | 67‰ | 100‰, under |
| lumber_yard | `building_lumbermill_blue` | 64‰ | 100‰, under |
| ore_pit | `building_mine_blue` | 5‰ | 100‰, under |
| muster_hall | `building_barracks_blue` | 84‰ | 100‰, under |
| watchtower | `building_tower_A_blue` | 81‰ | 100‰, under |
| engine_yard | `building_blacksmith_blue` | 91‰ | 100‰, under |
| croft | `building_home_A_blue` | 258‰ | 100‰, passes |
| scriptorium | `building_church_blue` | 280‰ | 100‰, passes |
| mangonel (unit) | `building_tower_catapult_blue`, catapult nodes | about 25‰ | 200‰, under |
| Seat flag | `flag_blue` | 457‰ | 300‰, passes |

The rect cannot be widened: it is already every texel that differs between KayKit's blue and red variants, so these buildings simply carry little team surface. The owner chose a team banner on every team building and on the Mangonel (owner decision 12 of the art pass, 2026-10-28; a base disc, lower floors and other models were declined), with the floor measured on model plus banner: the "Building looks" table above has the result, and every building reaches the floor (the Town and the Ore Pit with banners taller than the slice 3 cap, which the 2026-10-28 review dropped as not the owner's). The Mangonel carries the same banner since slice 4 ("Unit looks (art slice 4)" below).

`inspect_gltf palette <file> [--grid 16]` prints the base texture's cell grid: each used cell's texel rect, mean colour and the nodes whose faces sample it. It is how a character's `team_swatches` rects are picked (art slice 4), since the Adventurers characters ship in one colour and have no red variant for `swatches` to diff: run it on the source file (`--grid 8` gives the 128 x 256 px colour cells of these atlases) and divide the rects by the bake's downsample (4 for the 256 px bakes).

## Screenshot sign-off checklist

Attach one 1280x720 screenshot at 40 m to every PR that adds or changes art, and tick each line.

| Check | Yes |
|---|---|
| Every unit kind on screen is identifiable by silhouette alone | |
| Team colour is the only strong hue on units and buildings | |
| No surface shows specular highlight, glow or bloom | |
| Owned land reads as a soft tint with a visible but not hard border | |
| HUD panels are matte and text is legible at 100% scale | |
| The same screenshot looks matte on P3 and as an sRGB export | |
| Total hue count on screen, excluding team colours, is at most 12 | |
| New files have `ATTRIBUTION.md` rows and `scripts/check_assets.sh` passes | |
| `visuals.ron` changed, Rust did not (for a model swap) | |
| Draw-call counter in the dev panel is under 500 with the test scene | |

The last row had no counter to read through M4b: Bevy 0.19.1 keeps its batch counts `pub(crate)` and wgpu has no draw counters, so it was marked n/a in the PR with the expected counts instead (at 200 vs 200 about 75 to 100 batches, [design/ui.md](design/ui.md#health-bars-rings-and-markers)), and an Xcode Metal frame capture on the Mac stood in for it. The art pass's counter (`--draw-stats`, the dev panel) reads it: `art_bench` with every building, site, banner and terrain prop as a model peaks at 111 draws under lavapipe (art slice 3), and still 111 with every unit as a model too (art slice 4, framed at 40 m on P0's Town).

## Milestones

| Milestone | Visual deliverable |
|---|---|
| M0 | Flat matte ground plane, directional light, camera stub |
| M2 | Team-coloured primitives, gizmo rings and markers |
| M3b | Territory tint and border; build ghost in green or red |
| M4b | Five distinct unit silhouettes; health bars |
| M5a | Attrition indicator, supply ring, annexation timer |
| M7 | Fog through the field shader, minimap, Inter, Kenney panels, CC0 glTF models, audio; palette finalized here |
