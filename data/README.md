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
| `rules/rules.ron` | Match rules: tick rate, command delay, Yield Cap table, pop cap, town radii, attrition, annexation, path budget | `crates/rules/src/lib.rs` (`Rules`) | M0 |
| `rules/resources.ron` | The four resources: Grain, Lumber, Ore, Lore | `crates/rules/src/resources.rs` | M0 |
| `maps/plains_1v1.ron` | The one 128x128 mirrored two-player map | `crates/rules/src/map.rs` | M0 |
| `rules/ages.ron`, `units.ron`, `buildings.ron`, `techs.ron`, `factions.ron` | Content tables | not yet written | M3a, M4a |
| `rules/strings/en.ron` | UI strings | not yet written | M3b |
| `visuals.ron` | Visual kind to primitive or `.glb` mapping | not yet written | M2 |
| `ai/` | Build orders, personalities, difficulty table | not yet written | M5b |

Each subdirectory has its own README with the schema in plain English and a
worked example: [rules/README.md](rules/README.md),
[maps/README.md](maps/README.md), [ai/README.md](ai/README.md). The format
reference for contributors is [../docs/DATA_FORMAT.md](../docs/DATA_FORMAT.md).

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
error: data/rules/rules.ron: 7:3-7:18: Unexpected field named `tick_rate_hertz` in `Rules`, expected one of `rules_version`, `tick_rate_hz`, ... instead
```

## License

Everything under `data/` is licensed like the code: MIT OR Apache-2.0
(see [../LICENSE-MIT](../LICENSE-MIT) and [../LICENSE-APACHE](../LICENSE-APACHE)).
