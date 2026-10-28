//! Node-name globs, as `visuals.ron`'s `nodes` and `bake.ron`'s `keep`
//! use them: `*` matches any run of characters (including none), every
//! other character matches itself, and the whole name must match. A list
//! keeps a node when any glob matches its own name; an empty list keeps
//! every node. Keeping a node keeps its mesh only, not its children.

/// Whether `name` matches `pattern` (`rules::visuals::glob_match`, the
/// definition the game's model merge shares).
pub fn glob_match(pattern: &str, name: &str) -> bool {
    rules::visuals::glob_match(pattern, name)
}

/// Whether a node called `name` is kept by `globs` (empty keeps all).
pub fn keeps(globs: &[String], name: &str) -> bool {
    rules::visuals::keeps_node(globs, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn star_matches_any_run_and_the_rest_is_literal() {
        assert!(glob_match("Knight_*", "Knight_Body"));
        assert!(glob_match("Knight_*", "Knight_"));
        assert!(!glob_match("Knight_*", "Knight"));
        assert!(glob_match("*atapult*", "building_catapult_arm"));
        assert!(glob_match("*", ""));
        assert!(glob_match("1H_Sword", "1H_Sword"));
        assert!(!glob_match("1H_Sword", "1H_Sword_Offhand"));
        assert!(glob_match("a*b*c", "aXXbYYbc"));
        assert!(!glob_match("a*b*c", "aXXbYY"));
        assert!(keeps(&[], "anything"));
        assert!(!keeps(&["x".to_string()], "y"));
    }
}
