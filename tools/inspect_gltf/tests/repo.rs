//! Checks over the committed tree: `data/visuals.ron` against `assets/`,
//! and `tools/assets/sources.tsv` and `tools/assets/bake.ron` against
//! `assets/ATTRIBUTION.md`.

use inspect_gltf::sources::{parse_attribution, parse_sources, url_file_name};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `inspect_gltf check data assets` passes on the committed files (the CI
/// check job also runs the command).
#[test]
fn repo_models_pass_check() {
    let report =
        inspect_gltf::check::check_dirs(&root().join("data"), &root().join("assets")).unwrap();
    assert!(report.problems.is_empty(), "{}", report.text());
    let rules = rules::Rules::load(root().join("data")).unwrap();
    let scenes = rules
        .visuals
        .scene_paths()
        .into_iter()
        .filter(|(field, _)| !field.contains("parts["))
        .count();
    assert_eq!(report.scenes, scenes, "{}", report.text());
}

/// Every vendored row of sources.tsv and every output of bake.ron has an
/// ATTRIBUTION.md row whose Author, Source URL, License and Modifications
/// match (and a sha256 pin), and every third-party row of ATTRIBUTION.md
/// comes from one of them.
#[test]
fn attribution_rows_match_the_sources() {
    let tsv = std::fs::read_to_string(root().join("tools/assets/sources.tsv")).unwrap();
    let sources = parse_sources(&tsv).unwrap();
    let md = std::fs::read_to_string(root().join("assets/ATTRIBUTION.md")).unwrap();
    let rows = parse_attribution(&md);
    let find = |asset: &str| rows.iter().find(|r| r.asset == asset);
    let mut covered = BTreeSet::new();
    let date = |d: &str| {
        d.len() == 10
            && d.bytes().enumerate().all(|(i, b)| {
                if i == 4 || i == 7 {
                    b == b'-'
                } else {
                    b.is_ascii_digit()
                }
            })
    };
    for s in &sources {
        assert_eq!(s.sha256.len(), 64, "{} is not pinned", s.url);
        if s.licence_proof.is_some() {
            assert!(s.dest.is_some(), "licence text {} must be vendored", s.url);
        }
        let Some(dest) = &s.dest else { continue };
        let row = find(dest).unwrap_or_else(|| panic!("no ATTRIBUTION.md row for {dest}"));
        assert_eq!(row.author, s.author, "{dest}");
        assert_eq!(row.source_url, s.page, "{dest}");
        assert_eq!(row.license, s.licence, "{dest}");
        assert_eq!(row.original, "no", "{dest}");
        assert!(
            date(&row.download_date),
            "{dest}: date {:?}",
            row.download_date
        );
        let src_name = url_file_name(&s.url);
        let dest_name = url_file_name(dest);
        if src_name == dest_name {
            assert!(
                row.modifications == "none" || row.modifications.starts_with("none ("),
                "{dest}: a vendored file is byte for byte as shipped (scripts/check_assets.sh holds its bytes to the pin), so Modifications is none: {:?}",
                row.modifications
            );
        } else {
            assert_eq!(
                row.modifications,
                format!("renamed from `{src_name}`"),
                "{dest}"
            );
        }
        assert!(
            root().join("assets").join(dest).is_file(),
            "{dest} is not vendored"
        );
        covered.insert(dest.clone());
    }
    let bake = std::fs::read_to_string(root().join("tools/assets/bake.ron")).unwrap();
    for m in inspect_gltf::bake::parse(&bake).unwrap().models {
        // Pinned like a vendored file (scripts/check_assets.sh holds the
        // committed output to it).
        assert_eq!(m.sha256.len(), 64, "{} is not pinned", m.out);
        let row = find(&m.out).unwrap_or_else(|| panic!("no ATTRIBUTION.md row for {}", m.out));
        let src = sources
            .iter()
            .find(|s| url_file_name(&s.url) == m.src)
            .unwrap_or_else(|| panic!("{} is not in sources.tsv", m.src));
        assert_eq!(row.author, src.author, "{}", m.out);
        assert_eq!(row.source_url, src.page, "{}", m.out);
        assert_eq!(row.license, src.licence, "{}", m.out);
        assert_eq!(row.original, "no", "{}", m.out);
        assert!(
            date(&row.download_date),
            "{}: date {:?}",
            m.out,
            row.download_date
        );
        let model = inspect_gltf::model::Model::load(&root().join("assets").join(&m.out)).unwrap();
        let extras = inspect_gltf::bake::read_extras(&model)
            .expect("a baked file records its modifications");
        assert_eq!(row.modifications, extras.modifications, "{}", m.out);
        covered.insert(m.out.clone());
    }
    for r in rows.iter().filter(|r| r.original == "no") {
        assert!(
            covered.contains(&r.asset),
            "ATTRIBUTION.md row {} comes from neither sources.tsv nor bake.ron",
            r.asset
        );
    }
}
