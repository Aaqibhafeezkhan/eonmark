//! `tools/assets/sources.tsv` (what `scripts/fetch_assets.sh` downloads and
//! pins) and the rows of `assets/ATTRIBUTION.md`, so a test can hold the
//! two together.

/// One row of `sources.tsv`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceRow {
    /// Path under `assets/`, or `None` (`-`) for a tool-only input kept in
    /// the download cache.
    pub dest: Option<String>,
    /// The `https://raw.githubusercontent.com/...` download URL.
    pub url: String,
    /// The pinned sha256 (64 hex digits; empty only while pinning).
    pub sha256: String,
    /// SPDX licence id.
    pub licence: String,
    /// Author as the source gives it.
    pub author: String,
    /// The per-file page that goes into ATTRIBUTION.md's Source URL.
    pub page: String,
    /// On a licence row, the extended regex its text must match.
    pub licence_proof: Option<String>,
}

/// The header `sources.tsv` must start with.
pub const SOURCES_HEADER: &str = "dest\turl\tsha256\tlicence\tauthor\tpage\tlicence_proof";

/// Parse `sources.tsv`: the header, then tab-separated rows; blank lines
/// and lines starting with `#` are skipped.
pub fn parse_sources(text: &str) -> Result<Vec<SourceRow>, String> {
    let mut lines = text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty() && !l.starts_with('#'));
    match lines.next() {
        Some((_, h)) if h == SOURCES_HEADER => {}
        other => {
            return Err(format!(
                "sources.tsv: the first row must be the header {SOURCES_HEADER:?}, found {:?}",
                other.map(|(_, l)| l)
            ));
        }
    }
    let mut rows = Vec::new();
    for (n, line) in lines {
        let cells: Vec<&str> = line.split('\t').collect();
        if cells.len() != 7 {
            return Err(format!(
                "sources.tsv:{}: {} cells, expected 7",
                n + 1,
                cells.len()
            ));
        }
        let row = SourceRow {
            dest: (cells[0] != "-").then(|| cells[0].to_string()),
            url: cells[1].to_string(),
            sha256: cells[2].to_string(),
            licence: cells[3].to_string(),
            author: cells[4].to_string(),
            page: cells[5].to_string(),
            licence_proof: (!cells[6].is_empty()).then(|| cells[6].to_string()),
        };
        if !row.url.starts_with("https://raw.githubusercontent.com/") {
            return Err(format!(
                "sources.tsv:{}: url must be on https://raw.githubusercontent.com/",
                n + 1
            ));
        }
        if !row.sha256.is_empty()
            && !(row.sha256.len() == 64
                && row
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
        {
            return Err(format!(
                "sources.tsv:{}: sha256 must be 64 lowercase hex digits",
                n + 1
            ));
        }
        if let Some(d) = &row.dest {
            rules::visuals::check_asset_path(&format!("{d}.glb")).map_err(|_| {
                format!(
                    "sources.tsv:{}: dest {d:?} must be a relative path under assets/",
                    n + 1
                )
            })?;
        }
        rows.push(row);
    }
    Ok(rows)
}

/// One row of the table in `assets/ATTRIBUTION.md`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttributionRow {
    /// Path relative to `assets/` (backticks removed).
    pub asset: String,
    /// Author.
    pub author: String,
    /// Source URL.
    pub source_url: String,
    /// SPDX licence id.
    pub license: String,
    /// Modifications.
    pub modifications: String,
    /// Download date.
    pub download_date: String,
    /// `yes` or `no`.
    pub original: String,
}

fn cell(s: &str) -> String {
    s.trim().trim_matches('`').trim().to_string()
}

/// The rows of the first table whose header starts with `| Asset |`.
pub fn parse_attribution(md: &str) -> Vec<AttributionRow> {
    let mut rows = Vec::new();
    let mut in_table = false;
    for line in md.lines() {
        if !in_table {
            in_table = line.starts_with("| Asset |");
            continue;
        }
        if !line.starts_with('|') {
            if !rows.is_empty() {
                break;
            }
            continue;
        }
        let cells: Vec<&str> = line.trim().trim_matches('|').split('|').collect();
        if cells.first().is_some_and(|c| c.trim().starts_with("---")) {
            continue;
        }
        if cells.len() != 7 {
            continue;
        }
        rows.push(AttributionRow {
            asset: cell(cells[0]),
            author: cells[1].trim().to_string(),
            source_url: cells[2].trim().to_string(),
            license: cells[3].trim().to_string(),
            modifications: cells[4].trim().to_string(),
            download_date: cells[5].trim().to_string(),
            original: cells[6].trim().to_string(),
        });
    }
    rows
}

/// The URL's last path segment.
pub fn url_file_name(url: &str) -> &str {
    url.rsplit('/').next().unwrap_or(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_and_attribution_rows_parse() {
        let tsv = format!(
            "# comment\n{SOURCES_HEADER}\nfonts/OFL-Inter.txt\thttps://raw.githubusercontent.com/rsms/inter/v4.1/LICENSE.txt\t{}\tOFL-1.1\tRasmus Andersson\thttps://github.com/rsms/inter/blob/v4.1/LICENSE.txt\tSIL OPEN FONT LICENSE\n-\thttps://raw.githubusercontent.com/a/b/main/x_red.bin\t\tCC0-1.0\tK\thttps://github.com/a/b/blob/main/x_red.bin\t\n",
            "a".repeat(64)
        );
        let rows = parse_sources(&tsv).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].dest.as_deref(), Some("fonts/OFL-Inter.txt"));
        assert_eq!(
            rows[0].licence_proof.as_deref(),
            Some("SIL OPEN FONT LICENSE")
        );
        assert_eq!(rows[1].dest, None);
        assert!(rows[1].sha256.is_empty());
        assert_eq!(url_file_name(&rows[0].url), "LICENSE.txt");
        assert!(parse_sources("dest\turl\n").is_err());
        let bad = tsv.replace(
            "https://raw.githubusercontent.com/rsms",
            "http://example.com/rsms",
        );
        assert!(parse_sources(&bad).is_err());
        let bad = tsv.replace(&"a".repeat(64), "ABC");
        assert!(parse_sources(&bad).is_err());
        let bad = tsv.replace("fonts/OFL-Inter.txt\t", "../x\t");
        assert!(parse_sources(&bad).is_err());

        let md = "# A\n\n| Asset | Author | Source URL | License | Modifications | Download date | Original |\n|---|---|---|---|---|---|---|\n| `fonts/x.ttf` | R | https://e/x | OFL-1.1 | none | 2026-10-27 | no |\n\nafter\n| not | a | row |\n";
        let rows = parse_attribution(md);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].asset, "fonts/x.ttf");
        assert_eq!(rows[0].license, "OFL-1.1");
        assert_eq!(rows[0].download_date, "2026-10-27");
    }
}
