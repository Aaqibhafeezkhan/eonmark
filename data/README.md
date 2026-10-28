# data/

This directory holds every tuning number, map and (from M5b) AI script that the
Eonmark simulation reads at start-up. Nothing in here is compiled into the
binary; `rules::Rules::load("data")` parses and validates it, and the
resulting `rules_hash` is written into every replay header so a changed file
is reported as `RULES CHANGED` rather than as a simulation bug. The two
presentation files, `visuals.ron` and `rules/strings/`, are validated too
but are not in `rules_hash`, since the simulation never reads them. The code
reads these files; it never defines gameplay constants of its own.

## Layout

| Path | Contents | Schema | Since |
|------|----------|--------|-------|
| `rules/rules.ron` | Match rules: tick rate, command delay, Yield Cap and pop cap bases, cost ramps, training queue, idle-worker seeking, standard start, town radii, growth age and limit, combat knobs, attrition, annexation, path budget | `crates/rules/src/lib.rs` (`Rules`) | M0 |
| `rules/resources.ron` | The four resources: Grain, Lumber, Ore, Lore | `crates/rules/src/resources.rs` | M0 |
| `rules/units.ron` | The eight unit kinds: movement, class, hit points, attack, cost and ramp, pop, training time, trainer, `requires_tech`, vision; then the counter table | `crates/rules/src/units.rs` (`Units`) | M1 (training fields M3a, combat M4a) |
| `rules/buildings.ron` | The eight building kinds: footprint, cost, build time, hit points, vision, base yield, gather slots, per-Town limit, border radius, attack (Town and Watchtower) | `crates/rules/src/buildings.rs` (`Buildings`) | M3a |
| `rules/techs.ron` | The 12 Scriptorium techs and, from M5a, the Watchtower's Harrying I and II (side techs that count toward no age): line, level, host, cost, research time, Modifier rows and `counts_toward_age` | `crates/rules/src/techs.rs` (`Techs`) | M4a, M5a |
| `rules/ages.ron` | The three ages: tech-count gate, cost and what each unlocks | `crates/rules/src/ages.rs` (`Ages`) | M4a |
| `rules/factions.ron` | The factions and their Modifier rows (Freeholders, none) | `crates/rules/src/factions.rs` (`Factions`) | M4a |
| `maps/plains_1v1.ron` | The one 128x128 mirrored two-player map | `crates/rules/src/map.rs` | M0 |
| `rules/strings/en.ron` | UI strings: the message line's text for every command rejection, HUD labels and tooltips. Presentation only, outside `rules_hash` (see `crates/rules/src/strings.rs`) | written | M3b |
| `visuals.ron` | How each unit and building kind is drawn: `Primitive(Capsule \| Cuboid \| Cylinder \| Sphere \| Cone)` and, from M4b, `Body(shape, height_tiles_x100, lying)`, and glTF `Scene`s from the art pass; keyed by unit kind id and (art pass) building kind id, every kind in `units.ron` and `buildings.ron` must appear; the art pass adds the sites, rubble, Seat flag, terrain props and team-colour swatches. Presentation only: validated by `Rules::load`, outside `rules_hash` since `rules_version` 25 ([ADR 0003](../docs/adr/0003-visuals-outside-rules-hash.md)) | `crates/rules/src/visuals.rs` (`Visuals`) | M2, M4b |
| `ai/personalities.ron` | One row per bot personality: its build order and every knob of the bot (cadence, caps, site policies, army mix, waves, defence and recall); M5b ships `standard`. Loaded last by `Rules::load`, attached as `Rules::ai` and in `rules_hash`, so an edit needs a `rules_version` bump | `crates/rules/src/ai.rs` (`AiRules`) | M5b |
| `ai/build_orders/*.ron` | One build order per file, its `id` equal to the file stem; M5b ships `standard.ron`. In `rules_hash` like the personalities | `crates/rules/src/ai.rs` (`AiRules`) | M5b |
| `ai/difficulty.ron` | Easy, Standard, Hard: income interval and aggression | not yet written | M6 |

Each subdirectory has its own README with the schema in plain English and a
worked example: [rules/README.md](rules/README.md),
[maps/README.md](maps/README.md), [ai/README.md](ai/README.md). The format
reference for contributors is [../docs/DATA_FORMAT.md](../docs/DATA_FORMAT.md).

## visuals.ron

Read by the game's presenter, never by the simulation, but loaded and
validated by `Rules::load`, so a missing or bad visual fails `sim-cli
data-check` and is caught before a window opens. Since `rules_version` 25
the file is not part of `rules_hash`
([ADR 0003](../docs/adr/0003-visuals-outside-rules-hash.md); before that it
was, from M2). Schema `crates/rules/src/visuals.rs`.

| Field | Meaning |
|-------|---------|
| `units` | Map from unit kind id (`units.ron` `id`) to a `Visual`. Every kind must have an entry; an unknown key is an error naming `units["<id>"]`. Two kinds of the same shape must differ by 1.4x in width, height or depth (one silhouette per kind). |
| `buildings` | Map from building kind id (`buildings.ron` `id`) to `Primitive(Cuboid)`, the M3b team-coloured box over the footprint, or a `Scene` whose fallback is that box (art pass). Every kind must have an entry; an unknown key is an error naming `buildings["<id>"]`. |
| `Primitive(Capsule)` / `Cuboid` / `Cylinder` / `Sphere` / `Cone` | One shared mesh per kind, sized from the kind's `radius_tiles_x100` (a Capsule 3.4 radii tall, the others 2 radii), tinted with the owner's team colour (M2; `Cone` from M4b). |
| `Body(shape, height_tiles_x100, lying)` | The same primitive two radii wide with its own height in hundredths of a tile (M4b): `height_tiles_x100` in 20..=400, a `Capsule` at least two radii. `lying: true` (default `false`, `Capsule` or `Cylinder` only) lays it along the unit's facing, `height_tiles_x100` long and two radii tall. A `Body` is never a `Sphere`. Errors name `units["<id>"].shape`, `.lying` or `.height_tiles_x100`. |
| `Scene(path, scale_x100, y_offset_tiles_x100, yaw_deg)` | glTF under `assets/` (`.glb` or `.gltf`, relative, no `..`), uniform scale in hundredths (1..=10000), vertical offset in hundredths of a tile, extra yaw in whole degrees (-360..=360). Optional: `x_offset_tiles_x100` and `z_offset_tiles_x100` (centre an off-centre file; every offset within ±500), `nodes` (node-name globs to keep), `parts` (more files in the same mesh, with an offset, scale and yaw), `team` (a `team_swatches` key whose rects are repainted in the owner's colour), `fallback` (what is drawn while the model is missing: a `Primitive` or `Body` for a unit, `Primitive(Cuboid)` for a building) and, on a building or a unit, `banner_at` (where its team banner stands) and on a Town `seat_flag_at` (where a Seat flies the Seat flag), each `(offset_x100: (x, y, z), scale_x100, yaw_deg)` from the footprint centre (a unit's position, in its own axes). The game draws building, site, rubble, flag and prop Scenes as models since art slice 3 and unit Scenes since slice 4; `sim-cli data-check` fails when the file is not under the assets directory. |
| `team_swatches`, `sites`, `rubble`, `seat_flag`, `banner`, `terrain`, `neutral_desaturate_permille` | Art-pass tables: texel rects per swatch key; 1..=4 construction stages (default the M3b rising box); the wreck of a destroyed building and how long it stays (`ds`); the flag on every Seat; the team banner model every `banner_at` plants (owner decision 12); props on `forest`, `mountain` and `water` tiles (never `grass`) with densities, jitter and weighted looks; how far every neutral model (props, sites, rubble) is desaturated toward grey, 0..=1000 permille. Field reference: [../docs/DATA_FORMAT.md](../docs/DATA_FORMAT.md); the shipped values: [../docs/ART_STYLE.md](../docs/ART_STYLE.md), "Building looks" and "Unit looks (art slice 4)". |

The game draws every look low-poly and flat-shaded. A changed look leaves
`rules_hash` alone, so it needs no `rules_version` bump and no `just
rerecord`: every replay still verifies (`OK`, not `RULES CHANGED`), and
`sim-cli data-check` still rejects an entry that breaks the rules above.
Anything the simulation or a bot reads does not belong in this file; it
goes in a hashed file under `rules/` or `ai/`.

```ron
(
  units: {
    "yeoman": Primitive(Capsule),
    "skirmisher": Body(shape: Cone, height_tiles_x100: 150),
    "outrider": Body(shape: Capsule, height_tiles_x100: 180, lying: true),
  },
  buildings: {
    "town": Primitive(Cuboid),
    "croft": Scene(path: "models/kaykit_hexagon/buildings/blue/building_home_A_blue.gltf", scale_x100: 223, y_offset_tiles_x100: 1, yaw_deg: 0, team: Some("kaykit_hexagon"), fallback: Some(Primitive(Cuboid)), banner_at: Some((offset_x100: (123, 0, 60), scale_x100: 250))),
  },
)
```

## Conventions

- Files are [RON](https://github.com/ron-rs/ron). Every struct uses
  `deny_unknown_fields`, so a misspelled field is an error that names the
  file and the field.
- Integers only. Time is authored in deciseconds in fields ending in `_ds`
  (`32` means 3.2 s). Distance is authored in tiles in fields ending in
  `_tiles`. The only conversion to ticks is `Rules::ticks_from_ds`:
  `ticks = round(ds * tick_rate_hz / 10)`.
- Names are Eonmark's own. See the naming rules in
  [../CONTRIBUTING.md](../CONTRIBUTING.md).

## Checking your change

```bash
cargo run -p sim-cli -- data-check data
cargo test -p rules
```

`data-check` exits 0 and prints the `rules_hash` on success (and, since the
art pass, checks that every glTF file `visuals.ron` names is under
`assets/`; `--assets <dir>` points it elsewhere, `--no-assets` skips it).
On failure it exits 1 and prints the file path, the span and the field, for
example:

```text
error: data/rules/rules.ron: 8:3-8:18: Unexpected field named `tick_rate_hertz` in `Rules`, expected one of `rules_version`, `tick_rate_hz`, ... instead
```

Every edit here except `visuals.ron` and `rules/strings/` changes
`rules_hash`, so a change you keep to any other file also bumps
`rules_version` in `rules/rules.ron` and re-records the golden fixtures
with `just rerecord` before `cargo test -p sim`; the full data-only path is
the worked example in [rules/README.md](rules/README.md). A look or UI
string edit needs only `data-check`: neither file is hashed, so it takes no
bump and no re-record.

## License

Everything under `data/` is licensed like the code: MIT OR Apache-2.0
(see [../LICENSE-MIT](../LICENSE-MIT) and [../LICENSE-APACHE](../LICENSE-APACHE)).
