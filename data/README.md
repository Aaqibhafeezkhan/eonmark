# data/

This directory holds every tuning number, map and (from M5b) AI script that the
Eonmark simulation reads at start-up. Nothing in here is compiled into the
binary; `rules::Rules::load("data")` parses and validates it, and the
resulting `rules_hash` is written into every replay header so a changed file
is reported as `RULES CHANGED` rather than as a simulation bug. The code
reads these files; it never defines gameplay constants of its own.

## Layout

| Path | Contents | Schema | Since |
|------|----------|--------|-------|
| `rules/rules.ron` | Match rules: tick rate, command delay, Yield Cap and pop cap bases, cost ramps, training queue, idle-worker seeking, standard start, town radii, growth age and limit, combat knobs, attrition, annexation, path budget | `crates/rules/src/lib.rs` (`Rules`) | M0 |
| `rules/resources.ron` | The four resources: Grain, Lumber, Ore, Lore | `crates/rules/src/resources.rs` | M0 |
| `rules/units.ron` | The eight unit kinds: movement, class, hit points, attack, cost and ramp, pop, training time, trainer, `requires_tech`, vision; then the counter table | `crates/rules/src/units.rs` (`Units`) | M1 (training fields M3a, combat M4a) |
| `rules/buildings.ron` | The eight building kinds: footprint, cost, build time, hit points, vision, base yield, gather slots, per-Town limit, border radius, attack (Town and Watchtower) | `crates/rules/src/buildings.rs` (`Buildings`) | M3a |
| `rules/techs.ron` | The 12 techs: line, level, host, cost, research time and Modifier rows | `crates/rules/src/techs.rs` (`Techs`) | M4a |
| `rules/ages.ron` | The three ages: tech-count gate, cost and what each unlocks | `crates/rules/src/ages.rs` (`Ages`) | M4a |
| `rules/factions.ron` | The factions and their Modifier rows (Freeholders, none) | `crates/rules/src/factions.rs` (`Factions`) | M4a |
| `maps/plains_1v1.ron` | The one 128x128 mirrored two-player map | `crates/rules/src/map.rs` | M0 |
| `rules/strings/en.ron` | UI strings: the message line's text for every command rejection, HUD labels and tooltips. Presentation only, outside `rules_hash` (see `crates/rules/src/strings.rs`) | written | M3b |
| `visuals.ron` | How each unit kind is drawn: `Primitive(Capsule \| Cuboid \| Cylinder \| Sphere \| Cone)` and, from M4b, `Body(shape, height_tiles_x100, lying)` now, `Scene(path, scale_x100, y_offset_tiles_x100, yaw_deg)` from M7; keyed by unit kind id, every kind in `units.ron` must appear | `crates/rules/src/visuals.rs` (`Visuals`) | M2, M4b |
| `ai/` | Build orders, personalities, difficulty table | not yet written | M5b |

Each subdirectory has its own README with the schema in plain English and a
worked example: [rules/README.md](rules/README.md),
[maps/README.md](maps/README.md), [ai/README.md](ai/README.md). The format
reference for contributors is [../docs/DATA_FORMAT.md](../docs/DATA_FORMAT.md).

## visuals.ron

Read by the game's presenter, never by the simulation, but loaded and
validated by `Rules::load` so a missing visual is caught before a window
opens and so the file is part of `rules_hash`. Schema `crates/rules/src/visuals.rs`.

| Field | Meaning |
|-------|---------|
| `units` | Map from unit kind id (`units.ron` `id`) to a `Visual`. Every kind must have an entry; an unknown key is an error naming `units["<id>"]`. |
| `Primitive(Capsule)` / `Cuboid` / `Cylinder` / `Sphere` / `Cone` | One shared mesh per kind, sized from the kind's `radius_tiles_x100` (a Capsule 3.4 radii tall, the others 2 radii), tinted with the owner's team colour (M2; `Cone` from M4b). |
| `Body(shape, height_tiles_x100, lying)` | The same primitive two radii wide with its own height in hundredths of a tile (M4b): `height_tiles_x100` in 20..=400, a `Capsule` at least two radii. `lying: true` (default `false`, `Capsule` or `Cylinder` only) lays it along the unit's facing, `height_tiles_x100` long and two radii tall. A `Body` is never a `Sphere`. Errors name `units["<id>"].shape`, `.lying` or `.height_tiles_x100`. |
| `Scene(path, scale_x100, y_offset_tiles_x100, yaw_deg)` | glTF under `assets/` (`.glb` or `.gltf`), uniform scale in hundredths (1..=10000), vertical offset in hundredths of a tile, extra yaw in whole degrees (-360..=360). Parsed and validated now, used from M7. |

The game draws every look low-poly and flat-shaded. A changed look changes
`rules_hash`, so it needs a `rules_version` bump and `just rerecord` like
any other data edit; the sim never reads it, so every `.hash` file stays the
same and only the replay headers change.

```ron
(units: {
  "yeoman": Primitive(Capsule),
  "skirmisher": Body(shape: Cone, height_tiles_x100: 150),
  "outrider": Body(shape: Capsule, height_tiles_x100: 180, lying: true),
})
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

`data-check` exits 0 and prints the `rules_hash` on success. On failure it
exits 1 and prints the file path, the span and the field, for example:

```text
error: data/rules/rules.ron: 8:3-8:18: Unexpected field named `tick_rate_hertz` in `Rules`, expected one of `rules_version`, `tick_rate_hz`, ... instead
```

Every edit here changes `rules_hash`, so a change you keep also bumps
`rules_version` in `rules/rules.ron` and re-records the golden fixtures
with `just rerecord` before `cargo test -p sim`; the full data-only path is
the worked example in [rules/README.md](rules/README.md).

## License

Everything under `data/` is licensed like the code: MIT OR Apache-2.0
(see [../LICENSE-MIT](../LICENSE-MIT) and [../LICENSE-APACHE](../LICENSE-APACHE)).
