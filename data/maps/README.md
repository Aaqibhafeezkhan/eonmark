# data/maps/

Maps are hand-authored RON files, one per map, loaded and validated by
`rules::map::MapDef` as part of `Rules::load`. Every map under this directory
is loaded, so a broken file fails `data-check` even if no match uses it. v0.1
ships exactly one map, `plains_1v1`.

## Format

```text
(
  name: "plains_1v1",          // must equal the file stem
  width: 128,                  // even, >= 8
  height: 128,                 // >= 8
  symmetry: MirrorX,           // MirrorX or Rotate180
  starts: [(x: 24, y: 64), (x: 103, y: 64)],   // exactly 2, on grass
  rows: [                      // height strings of width characters
    "....f...",
    ...
  ],
)
```

| Char | Terrain | Passable | Resource |
|------|---------|----------|----------|
| `.` | Grass | yes | none; the only buildable terrain |
| `f` | Forest | no | Lumber: a Lumber Yard beside it gets one gather slot per adjacent forest tile (max 6) |
| `m` | Mountain | no | Ore: an Ore Pit beside it gets one slot per adjacent mountain tile (max 6) |
| `~` | Water | no | none |

Row 0 is the north edge, column 0 is the west edge; `x` grows east and `y`
grows south. These are the simulation's tile coordinates.

## Symmetry

`MirrorX` means tile `(x, y)` equals tile `(width - 1 - x, y)`: the east half
is the west half flipped across the vertical midline, which runs between
columns `width/2 - 1` and `width/2`. `Rotate180` means `(x, y)` equals
`(width - 1 - x, height - 1 - y)`. The validator checks every tile and also
requires `starts[1]` to be the symmetry partner of `starts[0]`, so both
players see the same distances to the same resources.

## plains_1v1

128x128, `MirrorX`. A river four tiles wide runs down the midline with three
fords (rows 14 to 21, 60 to 67 and 106 to 113). Each side has a start on
open grass at `(24, 64)` and `(103, 64)`, a forest and a mountain clump
within about 15 tiles, more forests and mountains towards the midline, and
two small lakes. Tile counts: 14758 grass, 704 forest, 390 mountain, 532
water.

## Validation rules

- `width` is even and both dimensions are >= 8.
- `rows.len() == height` and every row has exactly `width` characters from
  the table above. Errors name the row: `rows[17]`.
- Exactly 2 starts, inside the map, on grass, distinct, and mirror partners.
- Symmetry holds for every tile; the first mismatch names the row, column
  and partner.
- `name` equals the file stem.

```bash
cargo run -p sim-cli -- data-check data
cargo test -p rules map
```

Adding a second map is a data-only change (M9 and later): drop a file here
and point `default_map` in `rules/rules.ron` at it or select it from the
menu once the menu supports map choice.
