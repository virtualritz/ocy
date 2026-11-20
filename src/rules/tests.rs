use super::{ALL_RULE_NAMES, SCANNED_HIDDEN_DIRS, scanned_hidden, select_rules, standard_rules};
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

/// The default rules, as a scan with no `-r` and no group flag gets them.
fn default_rules() -> eyre::Result<Vec<Rule>> {
    Ok(select_rules(standard_rules()?, &[], false, false)?)
}

/// The rules selected by `-r` values, with both group flags left off.
fn selected_rules(selectors: &[&str]) -> eyre::Result<Vec<Rule>> {
    let selectors: Vec<String> = selectors
        .iter()
        .map(|selector| (*selector).into())
        .collect();

    Ok(select_rules(standard_rules()?, &selectors, false, false)?)
}

fn names(rules: &[Rule]) -> Vec<String> {
    rules.iter().map(|rule| rule.name.to_string()).collect()
}

#[test]
fn the_built_in_rule_set_is_valid() -> eyre::Result<()> {
    assert!(!standard_rules()?.is_empty());
    Ok(())
}

/// A name is what `-r` takes, and a value is chained with commas, so no name may contain a
/// character that would need quoting or split the value.
#[test]
fn every_rule_name_is_a_single_token() -> eyre::Result<()> {
    for name in names(&standard_rules()?) {
        assert!(
            name.chars().all(|character| character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '-'),
            "{name:?} is not a single lowercase token"
        );
    }
    Ok(())
}

/// The catalog and the selector-validated name list drift apart easily, and a name missing
/// from the list would be unreachable from `-r`.
#[test]
fn the_rule_catalog_matches_the_listed_names() -> eyre::Result<()> {
    let catalog: std::collections::HashSet<String> =
        names(&standard_rules()?).into_iter().collect();
    let listed: std::collections::HashSet<String> = ALL_RULE_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect();

    assert_eq!(listed, catalog);
    Ok(())
}

#[test]
fn command_rules_are_opt_in() -> eyre::Result<()> {
    let without = default_rules()?.len();
    let with = select_rules(standard_rules()?, &[], false, true)?.len();

    assert_eq!(
        without + 1,
        with,
        "--allow-commands should add the Make rule"
    );
    Ok(())
}

#[test]
fn no_command_rule_is_present_by_default() -> eyre::Result<()> {
    let names = names(&default_rules()?);

    assert!(!names.contains(&"make".to_string()), "got {names:?}");
    Ok(())
}

/// A shared cache belongs to every project on the machine, so a scan of one directory
/// must not reclaim it without being asked.
#[test]
fn cache_rules_are_opt_in() -> eyre::Result<()> {
    let default = names(&default_rules()?);

    assert!(
        select_rules(standard_rules()?, &[], true, false)?.len() > default.len(),
        "--caches should add rules"
    );
    assert!(
        !default.contains(&"tool-caches".to_string()),
        "got {default:?}"
    );
    Ok(())
}

/// Naming a cache rule is the opt-in, so `-r apt-cache` works without `--caches`.
#[test]
fn naming_a_cache_rule_selects_it_without_the_flag() -> eyre::Result<()> {
    let mut selected = names(&selected_rules(&["cargo-registry"])?);
    selected.dedup();

    assert_eq!(vec!["cargo-registry"], selected);
    Ok(())
}

/// The group is the whole point of the flag: `-r caches` and `--caches` reach the same
/// rules, one by selecting only them and the other by adding them to the default set.
#[test]
fn the_caches_group_selects_every_cache_rule() -> eyre::Result<()> {
    let mut group = names(&selected_rules(&["caches"])?);
    group.sort_unstable();
    group.dedup();

    let mut expected: Vec<String> = super::CACHE_RULE_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    expected.sort_unstable();

    assert_eq!(expected, group);
    assert!(
        select_rules(standard_rules()?, &[], true, false)?
            .iter()
            .all(|rule| !super::is_cache_rule(&rule.name)
                || expected.contains(&rule.name.to_string()))
    );
    Ok(())
}

/// Every cache rule names the one directory it applies to. A marker-matched cache rule
/// would be free to fire against any project directory that happened to look similar.
#[test]
fn every_cache_rule_is_anchored_to_a_path() -> eyre::Result<()> {
    let unanchored: Vec<String> = selected_rules(&["caches"])?
        .into_iter()
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
    let anchored: Vec<String> = default_rules()?
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
        default_rules()?,
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
        default_rules()?,
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
        default_rules()?,
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
    let rules = select_rules(standard_rules()?, &[], true, false)?;
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
    let default = scanned_hidden(&default_rules()?);

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
        vec![Rule::remove_at("tool-caches", cache, &["sccache"])?],
    )?;

    assert_eq!(vec![".cache/sccache"], found);
    Ok(())
}

/// The apt rule has to reclaim what `apt-get clean` does and keep what it keeps: the lock
/// and the `partial` directory apt downloads into.
#[test]
fn the_apt_cache_rule_reclaims_what_apt_get_clean_does() -> eyre::Result<()> {
    let root = tempfile::tempdir()?;
    let apt = root.path().join("var/cache/apt");
    fs::create_dir_all(apt.join("archives/partial"))?;
    for file in [
        "archives/git_2.43.0_amd64.deb",
        "archives/lock",
        "archives/partial/zstd_1.5.5_amd64.deb.FAILED",
        "pkgcache.bin",
        "srcpkgcache.bin",
    ] {
        fs::write(apt.join(file), b"")?;
    }
    // A mirror elsewhere that happens to share the layout is not apt's cache.
    fs::create_dir_all(root.path().join("mirror/apt/archives"))?;
    fs::write(root.path().join("mirror/apt/archives/git.deb"), b"")?;

    let found = reclaimed_at(root.path(), vec![super::apt_cache_rule(apt.clone())?])?;

    assert_eq!(
        vec![
            "var/cache/apt/archives/git_2.43.0_amd64.deb",
            "var/cache/apt/archives/partial/zstd_1.5.5_amd64.deb.FAILED",
            "var/cache/apt/pkgcache.bin",
            "var/cache/apt/srcpkgcache.bin",
        ],
        found
    );
    assert!(apt.join("archives/lock").exists());
    Ok(())
}

/// The apt cache is shared by the whole machine, so it is absent from a default scan --
/// but naming it is the opt-in, and both spellings have to reach it.
#[test]
fn the_apt_cache_rule_needs_asking_for_by_name_or_group() -> eyre::Result<()> {
    let is_apt = |rule: &Rule| rule.anchor() == Some(Path::new(super::APT_CACHE_DIR));

    assert!(!default_rules()?.iter().any(is_apt));
    assert!(
        select_rules(standard_rules()?, &[], true, false)?
            .iter()
            .any(is_apt)
    );
    assert!(selected_rules(&["apt-cache"])?.iter().any(is_apt));
    assert!(selected_rules(&["apt"])?.iter().any(is_apt));
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

    let rule =
        super::moved_cache_rule("tool-caches", &moved).expect("a nested path yields a rule")?;
    let found = reclaimed_at(root.path(), vec![rule])?;

    assert_eq!(vec!["scratch/sccache"], found);
    Ok(())
}

/// A cache linked onto another disk has to be reclaimed where the link points: the walk
/// reaches the real directory from there, and the link itself frees nothing.
#[cfg(unix)]
#[test]
fn a_linked_cache_is_reclaimed_where_it_really_lives() -> eyre::Result<()> {
    let root = tempfile::tempdir()?;
    let root_path = root.path().canonicalize()?;
    let cache = root_path.join("home/.cache");
    let real = root_path.join("work/xdg/sccache");
    fs::create_dir_all(real.join("0"))?;
    fs::create_dir_all(&cache)?;
    std::os::unix::fs::symlink(&real, cache.join("sccache"))?;

    let rules = super::anchored("tool-caches", cache, &["sccache"])?;
    let found = reclaimed_at(&root_path, rules)?;

    assert_eq!(vec!["work/xdg/sccache"], found);
    Ok(())
}

/// Scanning the home directory meets a linked cache only as the link, so the link has to
/// be followed to its directory -- and the link itself left alone either way.
#[cfg(unix)]
#[test]
fn a_linked_cache_is_followed_out_of_the_scanned_tree() -> eyre::Result<()> {
    let root = tempfile::tempdir()?;
    let root_path = root.path().canonicalize()?;
    let home = root_path.join("home");
    let real = root_path.join("work/sccache");
    fs::create_dir_all(real.join("0"))?;
    fs::create_dir_all(home.join(".cache"))?;
    std::os::unix::fs::symlink(&real, home.join(".cache/sccache"))?;

    let rules = || Rule::remove_at("tool-caches", home.join(".cache"), &["sccache"]);
    let follow = |follow_links| -> eyre::Result<Vec<String>> {
        let options = WalkOptions {
            walk_all: true,
            follow_links,
            ..Default::default()
        };
        let start = RealFileSystem.directory_from_current(Some(&home))?;
        let notifier = SilentNotifier::default();
        Walker::new(RealFileSystem, vec![rules()?], &notifier, options).walk_from_path(&start);

        Ok(notifier
            .to_remove
            .into_inner()
            .expect("notifier lock")
            .into_iter()
            .filter_map(|candidate| match candidate.action {
                RemovalAction::Delete { file_info, .. } => {
                    Some(file_info.path.display().to_string())
                }
                RemovalAction::RunCommand { .. } => None,
            })
            .collect())
    };

    assert_eq!(vec![real.display().to_string()], follow(true)?);
    assert!(follow(false)?.is_empty());
    Ok(())
}

/// `~/.npm` is as likely to be the link as the cache inside it, so a link partway along
/// a target has to be followed too.
#[cfg(unix)]
#[test]
fn a_cache_behind_a_linked_parent_is_followed() -> eyre::Result<()> {
    let root = tempfile::tempdir()?;
    let root_path = root.path().canonicalize()?;
    let home = root_path.join("home");
    let real = root_path.join("work/npm");
    fs::create_dir_all(real.join("_cacache"))?;
    fs::create_dir_all(real.join("_logs"))?;
    fs::create_dir_all(&home)?;
    std::os::unix::fs::symlink(&real, home.join(".npm"))?;

    let options = WalkOptions {
        walk_all: true,
        follow_links: true,
        ..Default::default()
    };
    let rule = Rule::remove_at("tool-caches", home.clone(), &[".npm/_cacache"])?;
    let start = RealFileSystem.directory_from_current(Some(&home))?;
    let notifier = SilentNotifier::default();
    Walker::new(RealFileSystem, vec![rule], &notifier, options).walk_from_path(&start);

    let found: Vec<_> = notifier
        .to_remove
        .into_inner()
        .expect("notifier lock")
        .into_iter()
        .filter_map(|candidate| match candidate.action {
            RemovalAction::Delete { file_info, .. } => Some(file_info.path),
            RemovalAction::RunCommand { .. } => None,
        })
        .collect();

    assert_eq!(vec![real.join("_cacache")], found);
    Ok(())
}

/// `~/.cargo/registry` itself is as easily linked away as the cache inside it, and the
/// walk never enters a link, so the rule has to be reachable from outside it.
#[cfg(unix)]
#[test]
fn a_cache_under_a_linked_anchor_is_followed() -> eyre::Result<()> {
    let root = tempfile::tempdir()?;
    let root_path = root.path().canonicalize()?;
    let home = root_path.join("home");
    let real = root_path.join("work/registry");
    fs::create_dir_all(real.join("cache"))?;
    fs::create_dir_all(real.join("index"))?;
    fs::create_dir_all(home.join(".cargo"))?;
    std::os::unix::fs::symlink(&real, home.join(".cargo/registry"))?;

    let options = WalkOptions {
        walk_all: true,
        follow_links: true,
        ..Default::default()
    };
    let rules = super::anchored("cargo-registry", home.join(".cargo/registry"), &["cache"])?;
    let start = RealFileSystem.directory_from_current(Some(&home))?;
    let notifier = SilentNotifier::default();
    Walker::new(RealFileSystem, rules, &notifier, options).walk_from_path(&start);

    let found: Vec<_> = notifier
        .to_remove
        .into_inner()
        .expect("notifier lock")
        .into_iter()
        .filter_map(|candidate| match candidate.action {
            RemovalAction::Delete { file_info, .. } => Some(file_info.path),
            RemovalAction::RunCommand { .. } => None,
        })
        .collect();

    assert_eq!(vec![real.join("cache")], found);
    Ok(())
}

/// A variable pointing at a filesystem root names nothing a rule could reclaim, and must
/// not be turned into one that would try.
#[test]
fn a_moved_cache_needs_a_parent_to_anchor_on() {
    assert!(super::moved_cache_rule("tool-caches", Path::new("/")).is_none());
}

/// Every cache home in play has to be covered, or `--caches` finds nothing on a platform
/// whose tools do not use the one we guessed.
#[test]
fn a_rule_is_anchored_at_every_cache_home() -> eyre::Result<()> {
    let home = super::home_directory().expect("tests run with a home directory");
    let rules = select_rules(standard_rules()?, &[], true, false)?;

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

    let rules = default_rules;

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

/// `--rule` is typed by hand, so its casing should not matter.
#[test]
fn a_selector_is_case_insensitive() {
    assert_eq!(Some(vec!["cargo"]), super::expand_rule_selector("cargo"));
    assert_eq!(Some(vec!["cargo"]), super::expand_rule_selector("CARGO"));
    assert_eq!(Some(vec!["cargo"]), super::expand_rule_selector("Cargo"));
}

/// The guarantee is for every rule, not only the one used as the example above. A rule
/// added later is covered by construction: expansion is done on the canonical name.
#[test]
fn every_rule_name_expands_in_any_case() -> eyre::Result<()> {
    for name in ALL_RULE_NAMES {
        for requested in [name.to_string(), name.to_uppercase(), name.to_lowercase()] {
            assert_eq!(
                Some(vec![*name]),
                super::expand_rule_selector(&requested),
                "{requested:?} did not select rule {name:?}"
            );
        }
    }
    Ok(())
}

/// The names before the slug rename remain accepted when quoted, because normalization
/// folds their spacing, dots, slashes and underscores into the canonical form.
#[test]
fn old_spaced_names_still_resolve() {
    let expand = |selector: &str| super::expand_rule_selector(selector);

    assert_eq!(Some(vec!["tool-caches"]), expand("Tool cache"));
    assert_eq!(Some(vec!["tool-caches"]), expand("Tool caches"));
    assert_eq!(Some(vec!["tool-caches"]), expand("tool_cache"));
    assert_eq!(Some(vec!["python-venv"]), expand("Python venv"));
    assert_eq!(Some(vec!["git-worktree"]), expand("Git worktree"));
    assert_eq!(Some(vec!["dotnet"]), expand(".NET"));
    assert_eq!(Some(vec!["flutter"]), expand("Flutter/Dart"));
    assert_eq!(Some(vec!["nodejs"]), expand("Node.js"));
    assert_eq!(Some(vec!["xcode"]), expand("XCode"));
}

/// `cargo` is the build tool; `rust` is the language someone scanning their own machine
/// is more likely to type. `apt` is the command everyone knows the cache by.
#[test]
fn short_aliases_resolve_to_the_canonical_rule() {
    assert_eq!(Some(vec!["cargo"]), super::expand_rule_selector("rust"));
    assert_eq!(Some(vec!["cargo"]), super::expand_rule_selector("Rust"));
    assert_eq!(Some(vec!["apt-cache"]), super::expand_rule_selector("apt"));
    assert_eq!(
        Some(vec!["python-venv"]),
        super::expand_rule_selector("venv")
    );
}

/// An alias must not turn into a match for every rule.
#[test]
fn an_alias_does_not_select_an_unrelated_rule() {
    assert_eq!(None, super::expand_rule_selector("nonesuch"));
    assert_ne!(Some(vec!["gradle"]), super::expand_rule_selector("rust"));
}

/// A mistyped selector is ordinary user error, so it has to name the closest rule and say
/// how the flag is written rather than just refusing.
#[test]
fn an_unknown_selector_suggests_the_nearest_rule() {
    let message = super::UnknownRule {
        selector: "toolcache".into(),
        suggestion: super::suggest_rule_selector("toolcache"),
    }
    .to_string();

    assert!(
        message.contains("unknown rule `toolcache`"),
        "got {message}"
    );
    assert!(
        message.contains("did you mean `tool-caches`"),
        "got {message}"
    );
    assert!(message.contains("-r cargo,nodejs"), "got {message}");
}

/// A selector that is nothing like any rule still explains the syntax.
#[test]
fn an_unrecognizable_selector_still_explains_the_flag() {
    let message = super::UnknownRule {
        selector: "zzzzzzzzzz".into(),
        suggestion: super::suggest_rule_selector("zzzzzzzzzz"),
    }
    .to_string();

    assert!(!message.contains("did you mean"), "got {message}");
    assert!(message.contains("`ocy --rules`"), "got {message}");
}
