//! `sim-cli data-check` end to end: the binary on the repo data and on
//! broken copies of it (the M4a data-check acceptance line). Copies live
//! under Cargo's per-target temporary directory.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_data() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let (from, to) = (entry.path(), dst.join(entry.file_name()));
        if from.is_dir() {
            copy_dir(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

/// A fresh copy of `data/` named after the test, with `edit` applied to the
/// text of `file`.
fn edited_copy(name: &str, file: &str, edit: impl FnOnce(String) -> String) -> PathBuf {
    // The process id keeps concurrent runs sharing CARGO_TARGET_TMPDIR apart.
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("data-check-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(&repo_data(), &dir);
    let path = dir.join(file);
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, edit(text)).unwrap();
    dir
}

fn data_check(dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sim-cli"))
        .arg("data-check")
        .arg(dir)
        .output()
        .expect("sim-cli runs")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn repo_data_passes_with_three_ages_eight_kinds_and_twelve_techs() {
    let out = data_check(&repo_data());
    let stdout = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}{}", text(&out.stderr));
    assert!(stdout.starts_with("OK rules_version="), "{stdout}");
    assert!(
        stdout.contains("units=8 buildings=8 ages=3 techs=12"),
        "{stdout}"
    );
    // M5a: `techs` counts the techs that count toward an age; the
    // Watchtower's Harrying I and II are the two side techs.
    assert!(
        stdout.contains(" techs=12 side_techs=2 factions=1 "),
        "{stdout}"
    );
}

/// Run data-check on a copy of `data/` with the first `from` in `file`
/// replaced by `to`: it exits 1 with one `error:` line naming `file` and
/// each of `words`.
fn rejects_edit(name: &str, file: &str, from: &str, to: &str, words: &[&str]) {
    let dir = edited_copy(name, file, |text| {
        assert!(text.contains(from), "{file} no longer contains {from:?}");
        text.replacen(from, to, 1)
    });
    let out = data_check(&dir);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{}{stderr}", text(&out.stdout));
    assert!(
        stderr.starts_with("error: ") && stderr.lines().count() == 1 && stderr.contains(file),
        "{stderr}"
    );
    for w in words {
        assert!(stderr.contains(w), "{w:?} missing: {stderr}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_harrying_bonus_age_that_does_not_exist_exits_1_naming_rules_ron() {
    rejects_edit(
        "bonus-age",
        "rules/rules.ron",
        r#"harrying_bonus_age: Some("charter"),"#,
        r#"harrying_bonus_age: Some("bronze"),"#,
        &["harrying_bonus_age: ", "\"bronze\" is not an age id"],
    );
}

#[test]
fn a_decreasing_age_gap_table_exits_1_naming_the_entry() {
    rejects_edit(
        "gap-table",
        "rules/rules.ron",
        "attrition_age_gap_hp: [1, 2],",
        "attrition_age_gap_hp: [2, 1],",
        &["attrition_age_gap_hp[1]: ", "never decreases"],
    );
}

#[test]
fn a_civilian_annexation_attacker_exits_1_naming_the_selector() {
    rejects_edit(
        "civilian-attacker",
        "rules/rules.ron",
        "annexation_attackers: [Class(Infantry), Class(Cavalry)],",
        "annexation_attackers: [Class(Infantry), Class(Civilian)],",
        &["annexation_attackers[1]: ", "Civilian"],
    );
}

#[test]
fn a_town_that_stays_on_annexation_exits_1_naming_buildings_ron() {
    rejects_edit(
        "town-stays",
        "rules/buildings.ron",
        "town: true,",
        "town: true,\n      stays_on_annexation: true,",
        &["buildings[0].stays_on_annexation: "],
    );
}

#[test]
fn removing_a_tech_referenced_by_ages_ron_exits_1_naming_file_and_field() {
    let dir = edited_copy("remove-tech", "rules/techs.ron", |text| {
        // Cut the whole `statecraft_2` entry, from its `(` line to `),`.
        let id = text
            .find(r#"id: "statecraft_2","#)
            .expect("techs.ron lists statecraft_2");
        let start = text[..id].rfind("    (\n").expect("entry opens");
        let close = "\n    ),\n";
        let end = id + text[id..].find(close).expect("entry closes") + close.len();
        let mut text = text;
        text.replace_range(start..end, "");
        assert!(!text.contains("statecraft_2"), "{text}");
        text
    });
    let out = data_check(&dir);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{}{stderr}", text(&out.stdout));
    assert!(
        stderr.starts_with("error: ")
            && stderr.contains("rules/ages.ron")
            && stderr.contains("ages[1].unlocks.techs[1]")
            && stderr.contains("statecraft_2"),
        "{stderr}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_mul_written_as_1_25_exits_1_naming_techs_ron() {
    let dir = edited_copy("float-mul", "rules/techs.ron", |text| {
        assert!(text.contains("op: Mul, value: 900"), "{text}");
        text.replacen("op: Mul, value: 900", "op: Mul, value: 1.25", 1)
    });
    let out = data_check(&dir);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{}{stderr}", text(&out.stdout));
    assert!(
        stderr.starts_with("error: ") && stderr.contains("rules/techs.ron"),
        "{stderr}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
