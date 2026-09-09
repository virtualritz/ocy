use super::{SCANNED_HIDDEN_DIRS, scanned_hidden, standard_rules};
use ocy_core::filesystem::{FileSystem, RealFileSystem};
use ocy_core::models::{FileInfo, RemovalAction, RemovalCandidate};
use ocy_core::rule::Rule;
use ocy_core::walker::{VCS_DIRS, WalkNotifier, WalkOptions, Walker};
use std::fs;
use std::path::Path;
use std::sync::Mutex;

#[derive(Default)]
struct SilentNotifier {
    to_remove: Mutex<Vec<RemovalCandidate>>,
}

impl WalkNotifier for &SilentNotifier {
    fn notify_entered_directory(&self, _dir: &FileInfo) {}

    fn notify_candidate_for_removal(&self, candidate: RemovalCandidate) {
        self.to_remove
            .lock()
            .expect("notifier lock")
            .push(candidate);
    }

    fn notify_fail_to_scan(&self, _dir: &FileInfo, _report: eyre::Report) {}

    fn notify_walk_finish(&self) {}
}

/// Run the default rule set over a real tree of `paths`, and report what it would
/// reclaim, relative to the root.
///
/// A trailing `/` makes a directory; anything else is an empty file. Going through the
/// walker rather than asking rules directly is what makes these tests worth having: it
/// covers whether a rule's directory is reached at all, not only whether it matches.
fn reclaimed(paths: &[&str], rules: Vec<Rule>) -> eyre::Result<Vec<String>> {
    let root = tempfile::tempdir()?;

    for path in paths {
        let full = root.path().join(path.trim_end_matches('/'));
        if path.ends_with('/') {
            fs::create_dir_all(&full)?;
        } else {
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&full, b"")?;
        }
    }

    reclaimed_at(root.path(), rules)
}

/// [`reclaimed`] for a tree that is already on disk.
///
/// The walk is told to enter hidden directories: the fixtures deliberately contain them,
/// and which of them a rule set opens by default is covered separately.
fn reclaimed_at(root: &Path, rules: Vec<Rule>) -> eyre::Result<Vec<String>> {
    let options = WalkOptions {
        walk_all: true,
        ..Default::default()
    };
    let start = RealFileSystem.directory_from_current(Some(root))?;
    let notifier = SilentNotifier::default();
    Walker::new(RealFileSystem, rules, &notifier, options).walk_from_path(&start);

    let mut found: Vec<String> = notifier
        .to_remove
        .into_inner()
        .expect("notifier lock")
        .into_iter()
        .filter_map(|candidate| match candidate.action {
            RemovalAction::Delete { file_info, .. } => Some(
                file_info
                    .path
                    .strip_prefix(root)
                    .unwrap_or(&file_info.path)
                    .display()
                    .to_string(),
            ),
            RemovalAction::RunCommand { .. } => None,
        })
        .collect();
    found.sort();
    Ok(found)
}

#[test]
fn the_built_in_rule_set_is_valid() -> eyre::Result<()> {
    assert!(!standard_rules(false, false)?.is_empty());
    Ok(())
}

#[test]
fn command_rules_are_opt_in() -> eyre::Result<()> {
    let without = standard_rules(false, false)?.len();
    let with = standard_rules(true, false)?.len();

    assert_eq!(
        without + 1,
        with,
        "--allow-commands should add the Make rule"
    );
    Ok(())
}

#[test]
fn no_command_rule_is_present_by_default() -> eyre::Result<()> {
    let names: Vec<String> = standard_rules(false, false)?
        .iter()
        .map(|rule| rule.name.to_string())
        .collect();

    assert!(!names.contains(&"Make".to_string()), "got {names:?}");
    Ok(())
}

/// A shared cache belongs to every project on the machine, so a scan of one directory
/// must not reclaim it without being asked.
#[test]
fn cache_rules_are_opt_in() -> eyre::Result<()> {
    let default: Vec<String> = standard_rules(false, false)?
        .iter()
        .map(|rule| rule.name.to_string())
        .collect();

    assert!(
        standard_rules(false, true)?.len() > default.len(),
        "--caches should add rules"
    );
    assert!(
        !default.contains(&"Tool cache".to_string()),
        "got {default:?}"
    );
    Ok(())
}

/// Every cache rule names the one directory it applies to. A marker-matched cache rule
/// would be free to fire against any project directory that happened to look similar.
#[test]
fn every_cache_rule_is_anchored_to_a_path() -> eyre::Result<()> {
    let default = standard_rules(false, false)?.len();

    let unanchored: Vec<String> = standard_rules(false, true)?
        .into_iter()
        .skip(default)
        .filter(|rule| rule.anchor().is_none())
        .map(|rule| rule.name.to_string())
        .collect();

    assert!(unanchored.is_empty(), "not anchored: {unanchored:?}");
    Ok(())
}

/// No project rule may be anchored: anchoring one to the machine it was written on would
/// stop it matching anywhere else.
#[test]
fn no_default_rule_is_anchored() -> eyre::Result<()> {
    let anchored: Vec<String> = standard_rules(true, false)?
        .into_iter()
        .filter(|rule| rule.anchor().is_some())
        .map(|rule| rule.name.to_string())
        .collect();

    assert!(anchored.is_empty(), "anchored: {anchored:?}");
    Ok(())
}

/// `CARGO_TARGET_DIR` moves and renames the target directory, leaving no `Cargo.toml`
/// beside it for the sibling rule to key on.
#[test]
fn a_renamed_cargo_target_directory_is_still_found() -> eyre::Result<()> {
    let found = reclaimed(
        &[
            "app/Cargo.toml",
            "app/target-alt/CACHEDIR.TAG",
            "app/target-alt/.rustc_info.json",
            "app/target-alt/debug/",
            // A cache tagged the same way, but not by cargo.
            "elsewhere/CACHEDIR.TAG",
        ],
        standard_rules(false, false)?,
    )?;

    assert_eq!(vec!["app/target-alt"], found);
    Ok(())
}

/// A CMake build directory is named by whoever configured it, and often sits beside the
/// source rather than inside it, so the only thing that reliably identifies one is the
/// cache file CMake writes into it.
#[test]
fn a_cmake_build_directory_is_found_whatever_it_is_called() -> eyre::Result<()> {
    let found = reclaimed(
        &[
            "proj/CMakeLists.txt",
            "proj/build_debug/CMakeCache.txt",
            "proj/Linux-x86_64-optimize/CMakeCache.txt",
            "proj/src/main.cpp",
        ],
        standard_rules(false, false)?,
    )?;

    assert_eq!(
        vec!["proj/Linux-x86_64-optimize", "proj/build_debug"],
        found
    );
    Ok(())
}

/// `dist` is keyed on `Trunk.toml` rather than on the manifest beside it, because a
/// `dist` next to a `Cargo.toml` is as often something the project keeps.
#[test]
fn a_dist_directory_is_only_claimed_for_a_trunk_project() -> eyre::Result<()> {
    let found = reclaimed(
        &[
            "wasm-app/Trunk.toml",
            "wasm-app/Cargo.toml",
            "wasm-app/dist/index.html",
            "packaged/Cargo.toml",
            "packaged/dist/release.tar.gz",
        ],
        standard_rules(false, false)?,
    )?;

    assert_eq!(vec!["wasm-app/dist"], found);
    Ok(())
}

/// Descending into version-control metadata is never useful, so the two lists must not
/// contradict each other.
#[test]
fn no_scanned_hidden_directory_is_version_control_metadata() {
    for hidden in SCANNED_HIDDEN_DIRS {
        assert!(
            !VCS_DIRS.contains(hidden),
            "{hidden} is both scanned and skipped"
        );
    }
}

#[test]
fn every_scanned_hidden_directory_is_actually_hidden() {
    for hidden in SCANNED_HIDDEN_DIRS {
        assert!(hidden.starts_with('.'), "{hidden} needs no allow-listing");
    }
}

/// Without this, `--caches` would silently need `--all` beside it to find anything.
#[test]
fn enabling_caches_opens_the_hidden_directories_they_live_in() -> eyre::Result<()> {
    let rules = standard_rules(false, true)?;
    let opened = scanned_hidden(&rules);

    for anchor in rules.iter().filter_map(Rule::anchor) {
        for component in super::hidden_components(anchor) {
            assert!(opened.contains(&component), "{component} stays unscanned");
        }
    }
    Ok(())
}

/// The default scan must not gain reach it did not have before.
#[test]
fn the_default_rule_set_opens_no_extra_hidden_directories() -> eyre::Result<()> {
    let default = scanned_hidden(&standard_rules(true, false)?);

    assert_eq!(SCANNED_HIDDEN_DIRS.len(), default.len(), "got {default:?}");
    Ok(())
}

/// A cache rule must reclaim its own anchor and nothing that merely looks like it.
#[test]
fn a_cache_rule_reclaims_only_the_cache_it_names() -> eyre::Result<()> {
    let root = tempfile::tempdir()?;
    let cache = root.path().join(".cache");
    fs::create_dir_all(cache.join("sccache"))?;
    fs::create_dir_all(root.path().join("project/.cache/sccache"))?;

    let found = reclaimed_at(
        root.path(),
        vec![Rule::remove_at("Tool cache", cache, &["sccache"])?],
    )?;

    assert_eq!(vec![".cache/sccache"], found);
    Ok(())
}

/// A tool's own variable is the one thing that reliably says where its cache went, so a
/// moved cache has to be reclaimed where it now lives rather than where it usually does.
#[test]
fn a_moved_cache_is_reclaimed_where_it_now_lives() -> eyre::Result<()> {
    let root = tempfile::tempdir()?;
    let moved = root.path().join("scratch/sccache");
    fs::create_dir_all(moved.join("0"))?;
    fs::create_dir_all(root.path().join("scratch/keep"))?;

    let rule = super::moved_cache_rule(&moved).expect("a nested path yields a rule")?;
    let found = reclaimed_at(root.path(), vec![rule])?;

    assert_eq!(vec!["scratch/sccache"], found);
    Ok(())
}

/// A variable pointing at a filesystem root names nothing a rule could reclaim, and must
/// not be turned into one that would try.
#[test]
fn a_moved_cache_needs_a_parent_to_anchor_on() {
    assert!(super::moved_cache_rule(Path::new("/")).is_none());
}

/// Every cache home in play has to be covered, or `--caches` finds nothing on a platform
/// whose tools do not use the one we guessed.
#[test]
fn a_rule_is_anchored_at_every_cache_home() -> eyre::Result<()> {
    let home = super::home_directory().expect("tests run with a home directory");
    let rules = standard_rules(false, true)?;

    for cache_home in super::cache_homes(&home) {
        assert!(
            rules.iter().any(|rule| rule.anchor() == Some(&*cache_home)),
            "no rule anchored at {}",
            cache_home.display()
        );
    }
    Ok(())
}

/// Nothing under `--caches` may name a directory that also holds installed software: the
/// cache inside it is reclaimable, the binaries beside it are not.
#[test]
fn no_cache_entry_names_a_directory_holding_installed_software() {
    for entry in super::HOME_ENTRIES {
        assert!(
            !matches!(
                *entry,
                ".npm" | ".local/share/pnpm" | ".bun" | ".m2" | ".ivy2"
            ),
            "{entry} holds more than a cache"
        );
    }
}

/// With no environment override, protection falls back to the ordinary `~/.config`,
/// `~/.local/share` and `~/.local/state` -- the locations `--all` would otherwise expose.
#[test]
fn protected_state_dirs_default_to_the_ordinary_xdg_locations() {
    let home = super::home_directory().expect("tests run with a home directory");
    let protected = super::protected_state_dirs();

    for default in [".config", ".local/share", ".local/state"] {
        let Ok(expected) = home.join(default).canonicalize() else {
            // Nothing has ever created this one on the machine running the test; there
            // is nothing under it that protecting it would change.
            continue;
        };
        assert!(
            protected.contains(&expected),
            "{} not protected: got {protected:?}",
            expected.display()
        );
    }
}

/// An installed application's own bundle can look exactly like a project -- Discord
/// ships a `package.json` beside each native module -- so this reproduces that shape and
/// checks it survives a `--all --caches` scan once it sits under a protected XDG
/// location, while confirming the same tree *would* otherwise be reclaimed.
#[test]
fn an_xdg_state_dir_is_never_reclaimed_even_under_walk_all() -> eyre::Result<()> {
    let root = tempfile::tempdir()?;
    let app = root.path().join("config/some-app");
    fs::create_dir_all(app.join("node_modules/dep"))?;
    fs::write(app.join("package.json"), b"{}")?;

    let rules = || standard_rules(false, false);

    let unprotected = reclaimed_at(root.path(), rules()?)?;
    assert_eq!(
        vec!["config/some-app/node_modules"],
        unprotected,
        "the fixture should look like an ordinary NodeJS project without protection"
    );

    let options = WalkOptions {
        walk_all: true,
        ignores: std::collections::HashSet::from([app.canonicalize()?]),
        ..Default::default()
    };
    let start = RealFileSystem.directory_from_current(Some(root.path()))?;
    let notifier = SilentNotifier::default();
    Walker::new(RealFileSystem, rules()?, &notifier, options).walk_from_path(&start);

    assert!(
        notifier
            .to_remove
            .into_inner()
            .expect("notifier lock")
            .is_empty(),
        "an ignored XDG state dir must not be reclaimed"
    );
    Ok(())
}
