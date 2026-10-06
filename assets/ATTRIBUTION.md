# Asset attribution

This file lists every file under `assets/`, original or third-party, with its author, source, license and download date, so that anyone redistributing Eonmark can honor the terms and so CI (`scripts/check_assets.sh`) can confirm that nothing is missing. Add or update a row in the same pull request that adds or changes an asset. The license of original assets is in [LICENSE-ASSETS.md](LICENSE-ASSETS.md). The terms you agree to when submitting an asset are in [../CONTRIBUTING.md](../CONTRIBUTING.md).

## Rows

| Asset | Author | Source URL | License | Modifications | Download date | Original |
|---|---|---|---|---|---|---|

No rows yet (M0). Rows are added with the pull request that adds the asset.

## Column rules

| Column | Rule |
|---|---|
| Asset | Path relative to `assets/`. One row per file, or one row per directory when a pack is vendored whole. |
| Author | Name as given at the source. For original files, the contributor's name or GitHub handle. |
| Source URL | The page the file was downloaded from. For original files, `n/a`. Never a fan wiki URL; that domain is grep-banned. |
| License | One SPDX id from the allowed table below. |
| Modifications | `none`, or what changed: recolored, decimated, converted to .glb, trimmed, resampled. |
| Download date | `YYYY-MM-DD`. For original files, the date the file was added. |
| Original | `yes` for a file made for Eonmark and dedicated under CC0-1.0 by LICENSE-ASSETS.md. `no` otherwise. |

## Allowed licenses

| License | SPDX id | Extra requirement |
|---|---|---|
| Creative Commons Zero 1.0 Universal | CC0-1.0 | A row here. |
| Creative Commons Attribution 4.0 | CC-BY-4.0 | A row here with author and source URL, so the credit travels with every redistribution. |
| SIL Open Font License 1.1 | OFL-1.1 | The license text must be shipped beside the font, for example `fonts/OFL-Inter.txt` next to `fonts/Inter-Variable.ttf`. OFL 1.1 requires it. |

Anything not in this table is not accepted.

## Forbidden

- CC-BY-SA, in any version. Share-alike terms conflict with the project's licenses.
- CC-BY-NC, in any version. NonCommercial terms are not open.
- GPL-licensed art or audio.
- Creative Commons Sampling Plus.
- Quaternius assets, which moved to a proprietary license on 2026-08-28.
- AI-generated art, audio, models, textures or fonts. See [../AI_CONTRIBUTIONS.md](../AI_CONTRIBUTIONS.md).
- Any text, table or image from the inspiration's fan wiki. Links to it fail CI (`scripts/check_trademark.sh`).

## Checks

`scripts/check_assets.sh` runs in CI and in `just ci`. It confirms that every file under `assets/` has a row or is on the original-file allowlist, that every license is in the allowed table, that every OFL font has its license text beside it, and that files stay under the size caps set in the script. Run it before committing an asset:

```bash
scripts/check_assets.sh
```
