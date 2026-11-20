use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ocy_core::rule::{Rule, RuleError, Target};

/// Hidden directories the walk descends into by default.
///
/// Only directories that can *contain* something reclaimable need listing. A hidden
/// directory that is itself a target -- `.next`, `.gradle`, `.terraform` -- is claimed by
/// its own rule without ever being descended into, and a nested target such as
/// `.angular/cache` is resolved directly rather than walked.
/// Registered git worktrees do not need listing here whatever they are called: the walk
/// follows each repository's own records to its checkouts. This list is only for hidden
/// directories that nothing else can lead the walk to.
pub const SCANNED_HIDDEN_DIRS: &[&str] = &[
    // Python virtual environments are self-marked, so the walk has to look inside.
    ".venv",
    // Checkouts parked here by hand, which have no git record to follow.
    ".worktrees",
];

/// The canonical name of every built-in rule.
///
/// A name is a single token -- lowercase, digits and hyphens -- because it is also the
/// value typed after `-r`, where anything containing a space would need quoting and could
/// not be chained with a comma.
mod names {
    pub const CARGO: &str = "cargo";
    pub const GRADLE: &str = "gradle";
    pub const MAVEN: &str = "maven";
    pub const SBT: &str = "sbt";
    pub const NODEJS: &str = "nodejs";
    pub const ANGULAR: &str = "angular";
    pub const PYTHON_VENV: &str = "python-venv";
    pub const PYTHON: &str = "python";
    pub const DOTNET: &str = "dotnet";
    pub const CMAKE: &str = "cmake";
    pub const ZIG: &str = "zig";
    pub const SWIFTPM: &str = "swiftpm";
    pub const XCODE: &str = "xcode";
    pub const ELIXIR: &str = "elixir";
    pub const CABAL: &str = "cabal";
    pub const STACK: &str = "stack";
    pub const FLUTTER: &str = "flutter";
    pub const UNITY: &str = "unity";
    pub const TRUNK: &str = "trunk";
    pub const TERRAFORM: &str = "terraform";
    pub const COMPOSER: &str = "composer";
    pub const GIT_WORKTREE: &str = "git-worktree";
    pub const MAKE: &str = "make";

    pub const CARGO_REGISTRY: &str = "cargo-registry";
    pub const CARGO_GIT: &str = "cargo-git";
    pub const GRADLE_CACHE: &str = "gradle-cache";
    pub const TOOL_CACHES: &str = "tool-caches";
    pub const APT_CACHE: &str = "apt-cache";
}
use names::*;

/// Every canonical rule name, in the order the [`standard_rules`] catalog builds them.
///
/// Kept as data rather than derived from the catalog so that a selector can be validated,
/// and a typo corrected, without building every rule first. A test holds the two in step.
const ALL_RULE_NAMES: &[&str] = &[
    CARGO,
    GRADLE,
    MAVEN,
    SBT,
    NODEJS,
    ANGULAR,
    PYTHON_VENV,
    PYTHON,
    DOTNET,
    CMAKE,
    ZIG,
    SWIFTPM,
    XCODE,
    ELIXIR,
    CABAL,
    STACK,
    FLUTTER,
    UNITY,
    TRUNK,
    TERRAFORM,
    COMPOSER,
    GIT_WORKTREE,
    MAKE,
    CARGO_REGISTRY,
    CARGO_GIT,
    GRADLE_CACHE,
    TOOL_CACHES,
    APT_CACHE,
];

/// Rules that are not ordinary project output, and so are left out of a default scan.
///
/// Naming one of them is what opts in: `-r cargo-registry` reclaims exactly that, and
/// `-r caches` selects the whole group.
const CACHE_RULE_NAMES: &[&str] = &[
    CARGO_REGISTRY,
    CARGO_GIT,
    GRADLE_CACHE,
    TOOL_CACHES,
    APT_CACHE,
];

/// Rules that run a command the scanned tree controls. Same opt-in rule as caches.
const COMMAND_RULE_NAMES: &[&str] = &[MAKE];

/// Groups a `-r` value may name instead of a single rule.
const RULE_GROUPS: &[(&str, &[&str])] = &[
    ("caches", CACHE_RULE_NAMES),
    ("commands", COMMAND_RULE_NAMES),
];

/// The built-in rule catalog, gated rules included.
///
/// Returning everything is safe because nothing runs or reaches outside the scan until a
/// rule is selected: [`select_rules`] holds back `make clean` and the shared caches unless
/// they are asked for, either by name or through their group.
pub fn standard_rules() -> Result<Vec<Rule>, RuleError> {
    // Alternative markers are separate rules, because a rule requires all of its markers.
    const PYTHON_CACHES: &[&str] = &[
        ".pytest_cache",
        ".mypy_cache",
        ".ruff_cache",
        ".tox",
        ".nox",
        ".hypothesis",
        "build",
        "dist",
    ];
    const DOTNET_OUTPUT: &[&str] = &["bin", "obj"];

    let rules = vec![
        // Rust. The second rule is what finds a target directory that `CARGO_TARGET_DIR`
        // has renamed or moved away from beside its manifest: cargo stamps every one it
        // creates with both of these, so the directory identifies itself.
        Rule::remove(CARGO, &["Cargo.toml"], &["target"])?,
        Rule::remove_self(CARGO, &["CACHEDIR.TAG", ".rustc_info.json"])?,
        // JVM. The glob covers both the Groovy and Kotlin build scripts.
        Rule::remove(GRADLE, &["build.gradle*"], &["build", ".gradle"])?,
        Rule::remove(MAVEN, &["pom.xml"], &["target"])?,
        Rule::remove(SBT, &["build.sbt"], &["target", "project/target"])?,
        // JavaScript and TypeScript.
        Rule::remove(
            NODEJS,
            &["package.json"],
            &[
                "node_modules",
                ".next",
                ".nuxt",
                ".turbo",
                ".svelte-kit",
                ".parcel-cache",
                // Only ever produced by pointing npm's cache at the project, which is
                // why the name is unambiguous where a bare `cache` would not be.
                ".npm-cache",
            ],
        )?,
        Rule::remove(ANGULAR, &["angular.json"], &[".angular/cache"])?,
        // Python. A virtual environment is identified by the `pyvenv.cfg` it contains,
        // which is why it needs a self-marked rule rather than a sibling one.
        Rule::remove_self(PYTHON_VENV, &["pyvenv.cfg"])?,
        Rule::remove(PYTHON, &["*.py"], &["__pycache__"])?,
        Rule::remove(PYTHON, &["pyproject.toml"], PYTHON_CACHES)?,
        Rule::remove(PYTHON, &["setup.py"], PYTHON_CACHES)?,
        // .NET.
        Rule::remove(DOTNET, &["*.csproj"], DOTNET_OUTPUT)?,
        Rule::remove(DOTNET, &["*.fsproj"], DOTNET_OUTPUT)?,
        // C and C++. A CMake build directory may be called anything -- `build`,
        // `build_debug`, `Linux-x86_64-optimize` -- and is as often a sibling of the
        // source as a child of it, so the sibling rule alone misses most of them.
        // `CMakeCache.txt` is written into the build directory itself, which makes the
        // directory self-marked whatever it was named.
        Rule::remove(CMAKE, &["CMakeLists.txt"], &["cmake-build-*", "CMakeFiles"])?,
        Rule::remove_self(CMAKE, &["CMakeCache.txt"])?,
        // Zig.
        Rule::remove(ZIG, &["build.zig"], &[".zig-cache", "zig-cache", "zig-out"])?,
        // Swift and Objective-C.
        Rule::remove(SWIFTPM, &["Package.swift"], &[".build"])?,
        Rule::remove(XCODE, &["*.xcodeproj"], &["DerivedData"])?,
        // Elixir.
        Rule::remove(ELIXIR, &["mix.exs"], &["_build", "deps"])?,
        // Haskell.
        Rule::remove(CABAL, &["*.cabal"], &["dist-newstyle"])?,
        Rule::remove(STACK, &["stack.yaml"], &[".stack-work"])?,
        // Dart and Flutter.
        Rule::remove(FLUTTER, &["pubspec.yaml"], &["build", ".dart_tool"])?,
        // Unity, which needs both markers to avoid matching any directory named `Assets`.
        Rule::remove(
            UNITY,
            &["Assets", "ProjectSettings"],
            &["Library", "Temp", "Obj", "Logs"],
        )?,
        // WebAssembly. Keyed on `Trunk.toml` rather than on the manifest beside it: a
        // `dist` next to a `Cargo.toml` is as likely to be something the project keeps.
        Rule::remove(TRUNK, &["Trunk.toml"], &["dist"])?,
        // Infrastructure.
        Rule::remove(TERRAFORM, &["*.tf"], &[".terraform"])?,
        // PHP.
        Rule::remove(COMPOSER, &["composer.json"], &["vendor"])?,
        // Git worktree records left behind by a deleted checkout.
        Rule::prune_stale_worktrees(GIT_WORKTREE, &[".git"])?,
    ];

    Ok(rules
        .into_iter()
        .chain([Rule::run(MAKE, &["Makefile"], "make clean")?])
        .chain(cache_rules()?)
        .collect())
}

/// Entries under a cache home that exist only to be a cache.
///
/// Every one of these is refetched or rebuilt on demand, so losing it costs time and
/// nothing else. A cache home also holds state that is not reproducible, which is why the
/// list is named out rather than inferred from the shape of what is in there.
///
/// Named generously across platforms: an anchored rule fires on an exact path, so an
/// entry that does not exist here -- `Mozilla.sccache` anywhere but macOS -- simply never
/// matches. That is cheaper and less brittle than encoding each tool's own idea of where
/// its cache belongs on which system.
const CACHE_HOME_ENTRIES: &[&str] = &[
    // Compiler caches.
    "sccache",
    "Mozilla.sccache",
    "ccache",
    "miri",
    // Rust tooling.
    "trunk",
    ".wasm-pack",
    // JavaScript tooling.
    "node-gyp",
    "pnpm",
    "yarn",
    "puppeteer",
    "deno",
    // Python tooling.
    "pip",
    "uv",
    // Go's build cache. Its *module* cache is deliberately absent: Go makes those
    // directories read-only, so removing one fails rather than reclaiming anything.
    // `go clean -modcache` is what handles that.
    "go-build",
];

/// Caches a tool keeps at a fixed place under the home directory instead.
///
/// Each entry names the cache itself rather than the directory holding it. `~/.npm` also
/// holds logs, and `~/.local/share/pnpm` also holds the binaries pnpm has installed --
/// reclaiming either wholesale would take out more than a cache.
const HOME_ENTRIES: &[&str] = &[
    // Where ccache kept its cache before 4.0, and still does when it finds one there.
    ".ccache",
    ".npm/_cacache",
    ".local/share/pnpm/store",
    ".bun/install/cache",
    ".m2/repository",
    ".ivy2/cache",
];

/// Environment variables that move a cache, and what to append to reach the cache itself.
///
/// A tool's own variable is the one thing that reliably says where its cache went;
/// reading its config file would mean a TOML parser for sccache, a second format for
/// ccache and a third for npm, each with its own way of being wrong.
///
/// The default location stays covered either way. A cache left behind at the old path is
/// still a cache, so these only add wherever the tool is being pointed now.
const MOVED_CACHES: &[(&str, &str)] = &[
    ("SCCACHE_DIR", ""),
    ("CCACHE_DIR", ""),
    ("UV_CACHE_DIR", ""),
    ("GOCACHE", ""),
    ("DENO_DIR", ""),
    ("npm_config_cache", "_cacache"),
    ("XDG_DATA_HOME", "pnpm/store"),
];

/// Where apt keeps the packages it has downloaded.
///
/// Fixed rather than read from apt's configuration, for the same reason cache locations
/// are not read from config files: `Dir::Cache` is almost never moved, and a parser for
/// apt's config format would be a lot of code to be wrong in for that case. Like every
/// anchored path, it simply never matches on a system without apt.
const APT_CACHE_DIR: &str = "/var/cache/apt";

/// What `apt-get clean` removes, relative to [`APT_CACHE_DIR`].
///
/// Every downloaded package, anything a download left half-finished, and the two binary
/// caches apt rebuilds from its package lists on its next run. The `archives/lock` file
/// and the `archives/partial` directory itself stay, as they do under `apt-get clean`.
/// The package lists in `/var/lib/apt/lists` are not a cache in this sense: without them
/// apt knows no packages at all until the next `apt update`.
const APT_CACHE_FILES: &[&str] = &["archives/*.deb", "pkgcache.bin", "srcpkgcache.bin"];
const APT_CACHE_PARTIAL: &str = "archives/partial/*";

/// The rule reclaiming apt's package cache at `anchor`.
///
/// The cache belongs to root, so only a scan run as root can actually remove anything;
/// any other scan still reports what is there, and each removal then fails on its own.
fn apt_cache_rule(anchor: PathBuf) -> Result<Rule, RuleError> {
    const NAME: &str = names::APT_CACHE;
    let invalid = |pattern: &str| {
        let pattern = pattern.to_string();
        move |source| RuleError::InvalidPattern {
            name: NAME.to_string(),
            pattern,
            source,
        }
    };

    let mut targets = APT_CACHE_FILES
        .iter()
        .map(|path| Target::file(path).map_err(invalid(path)))
        .collect::<Result<Vec<_>, _>>()?;
    targets.push(Target::any(APT_CACHE_PARTIAL).map_err(invalid(APT_CACHE_PARTIAL))?);

    Rule::remove_targets_at(NAME, anchor, targets)
}

/// Rules for the caches that tools keep once for the whole machine.
///
/// These are anchored to a path rather than matched by markers, because that is what
/// identifies them: a cache is a cache by virtue of where the tool agreed to put it, and
/// nothing inside `~/.cache/sccache` says so. Anchoring also means the rule cannot fire
/// against a project directory that happens to look similar.
///
/// Each location can be moved by an environment variable, and a scan that only knew the
/// default would quietly find nothing on a machine that had moved it.
///
/// An anchored rule is still only reached by walking, so moving a cache outside the tree
/// being scanned puts it out of reach whether or not its variable is set.
fn cache_rules() -> Result<Vec<Rule>, RuleError> {
    let Some(home) = home_directory() else {
        log::warn!("cannot locate the home directory, so no shared cache is known");
        return Ok(Vec::new());
    };

    let cargo = env_dir("CARGO_HOME").unwrap_or_else(|| home.join(".cargo"));
    let gradle = env_dir("GRADLE_USER_HOME").unwrap_or_else(|| home.join(".gradle"));

    let mut rules = [
        // Downloaded crates and the sources unpacked from them. `index` is left alone:
        // it is small, and refetching it stalls the next build of every project at once.
        anchored(CARGO_REGISTRY, cargo.join("registry"), &["cache", "src"])?,
        // Bare clones of git dependencies and the working copies checked out from them.
        anchored(CARGO_GIT, cargo.join("git"), &["checkouts", "db"])?,
        // Gradle's own `wrapper` directory is left alone: it holds the Gradle
        // distributions themselves, which is an installation rather than a cache.
        anchored(GRADLE_CACHE, gradle, &["caches", "daemon"])?,
        anchored(TOOL_CACHES, home.clone(), HOME_ENTRIES)?,
        vec![apt_cache_rule(PathBuf::from(APT_CACHE_DIR))?],
    ]
    .concat();

    for cache_home in cache_homes(&home) {
        rules.extend(anchored(TOOL_CACHES, cache_home, CACHE_HOME_ENTRIES)?);
    }

    for (variable, suffix) in MOVED_CACHES {
        let Some(moved) = env_dir(variable) else {
            continue;
        };
        let moved = if suffix.is_empty() {
            moved
        } else {
            moved.join(suffix)
        };

        match moved_cache_rule(TOOL_CACHES, &moved) {
            Some(rule) => rules.push(rule?),
            // A variable naming a filesystem root, or a path that is not valid UTF-8,
            // leaves nothing a rule could reclaim.
            None => log::warn!("ignoring {variable}: {} names no cache", moved.display()),
        }
        if let Some(rule) = linked_cache_rule(TOOL_CACHES, &moved) {
            rules.push(rule?);
        }
    }

    Ok(rules)
}

/// The cache homes in use on this platform.
///
/// `XDG_CACHE_HOME` wins where it is set, as it does for every tool that honours it.
/// macOS keeps its own cache directory, and tools there are split over which they use, so
/// both are in play on the same machine.
fn cache_homes(home: &Path) -> Vec<PathBuf> {
    let mut homes = vec![env_dir("XDG_CACHE_HOME").unwrap_or_else(|| home.join(".cache"))];

    if cfg!(target_os = "macos") {
        homes.push(home.join("Library/Caches"));
    }
    homes
}

/// A rule reclaiming `path` itself, anchored on the directory that holds it.
///
/// [`Rule::remove_at`] reclaims entries *inside* its anchor, so a cache that has been
/// moved somewhere of its own -- with no sibling worth naming -- is expressed by
/// anchoring one level up. Returns [`None`] for a path with no parent or no name.
fn moved_cache_rule(rule: &str, path: &Path) -> Option<Result<Rule, RuleError>> {
    let parent = path.parent()?;
    let name = path.file_name()?.to_str()?;

    Some(Rule::remove_at(rule, parent.to_path_buf(), &[name]))
}

/// Rules reclaiming `entries` under `anchor`, and wherever a symlink has put one of them.
///
/// A cache is often a link to a roomier disk -- `~/.cache/sccache` pointing into `/work`.
/// A scan that reaches the real directory directly has to know it by its real path, so
/// the directory each entry resolves to is named on its own.
///
/// The anchor itself may sit behind a link too -- `~/.cargo/registry` is as easily moved
/// as the cache inside it. The walk does not enter links, so such an anchor is never
/// reached; the rule is restated from just outside the link, where following it is left
/// to the walk.
fn anchored(rule: &str, anchor: PathBuf, entries: &[&str]) -> Result<Vec<Rule>, RuleError> {
    let mut rules = entries
        .iter()
        .filter_map(|entry| linked_cache_rule(rule, &anchor.join(entry)))
        .collect::<Result<Vec<_>, _>>()?;

    if let Some(outside) = outside_link(rule, &anchor, entries) {
        rules.push(outside?);
    }

    rules.insert(0, Rule::remove_at(rule, anchor, entries)?);
    Ok(rules)
}

/// `entries` of `anchor` restated from the directory holding its outermost symlinked
/// ancestor, or [`None`] when no ancestor is a link.
fn outside_link(rule: &str, anchor: &Path, entries: &[&str]) -> Option<Result<Rule, RuleError>> {
    let link = anchor
        .ancestors()
        .filter(|ancestor| {
            ancestor
                .symlink_metadata()
                .is_ok_and(|metadata| metadata.is_symlink())
        })
        .last()?;
    let parent = link.parent()?;
    let prefix = anchor.strip_prefix(parent).ok()?;

    let targets = entries
        .iter()
        .map(|entry| prefix.join(entry).to_str().map(str::to_string))
        .collect::<Option<Vec<_>>>()?;
    let targets: Vec<&str> = targets.iter().map(String::as_str).collect();

    Some(Rule::remove_at(rule, parent.to_path_buf(), &targets))
}

/// A rule reclaiming the directory `path` resolves to, when that is somewhere else.
///
/// Returns [`None`] when `path` does not exist, resolves to itself, or is not a directory.
fn linked_cache_rule(rule: &str, path: &Path) -> Option<Result<Rule, RuleError>> {
    let real = path.canonicalize().ok()?;

    if real == path || !real.is_dir() {
        None
    } else {
        moved_cache_rule(rule, &real)
    }
}

/// A directory named by an environment variable, treating an empty setting as unset.
fn env_dir(variable: &str) -> Option<PathBuf> {
    std::env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// The user's home directory, or [`None`] where there is no way to tell.
fn home_directory() -> Option<PathBuf> {
    std::env::home_dir()
}

/// Extract rule names from a slice of rules.
pub fn rule_names(rules: &[Rule]) -> HashSet<Arc<str>> {
    rules.iter().map(|rule| rule.name.clone()).collect()
}

/// Whether `name` is one of the shared-cache rules.
pub fn is_cache_rule(name: &str) -> bool {
    CACHE_RULE_NAMES.contains(&name)
}

/// Whether `name` is one of the rules that run a project's own command.
pub fn is_command_rule(name: &str) -> bool {
    COMMAND_RULE_NAMES.contains(&name)
}

/// Spellings accepted for a rule whose canonical name is not what people type.
///
/// Only names that [`normalize_rule_selector`] cannot bridge need an entry: `"Python
/// venv"` already becomes `python-venv`, so only a genuinely different word -- `rust` for
/// `cargo` -- is listed here.
const RULE_ALIASES: &[(&str, &str)] = &[
    ("rust", CARGO),
    ("net", DOTNET),
    ("node", NODEJS),
    ("node-js", NODEJS),
    ("apt", APT_CACHE),
    ("tool-cache", TOOL_CACHES),
    ("tools", TOOL_CACHES),
    ("venv", PYTHON_VENV),
    ("swift", SWIFTPM),
    ("dart", FLUTTER),
    ("flutter-dart", FLUTTER),
    ("git-worktrees", GIT_WORKTREE),
];

/// Fold a `-r` value to the form canonical names are written in.
///
/// Case, surrounding whitespace and the separators names are commonly written with are
/// all irrelevant: `"Tool cache"`, `"tool_caches"`, `"TOOL-CACHE"` and `"tool cache"`
/// fold to `tool-cache`. The shell cannot protect a space inside an argv token, so
/// accepting the spaced spelling when quoted costs nothing and keeps the pre-slug names
/// working.
fn normalize_rule_selector(selector: &str) -> String {
    let mut normalized = String::with_capacity(selector.len());
    let mut separator_pending = false;

    for character in selector.trim().chars() {
        if matches!(character, ' ' | '\t' | '.' | '/' | '_' | '-') {
            separator_pending = !normalized.is_empty();
        } else {
            if separator_pending {
                normalized.push('-');
                separator_pending = false;
            }
            normalized.extend(character.to_lowercase());
        }
    }

    normalized
}

/// The canonical name a folded selector names, if it is one rule.
fn canonical_rule_name(normalized: &str) -> Option<&'static str> {
    if let Some((_, canonical)) = RULE_ALIASES.iter().find(|(alias, _)| *alias == normalized) {
        return Some(canonical);
    }

    ALL_RULE_NAMES
        .iter()
        .copied()
        .find(|name| *name == normalized)
}

/// Every rule a single `-r` value selects, or [`None`] when it names nothing.
///
/// A group name expands to the rules in it; a rule name selects just itself.
pub fn expand_rule_selector(selector: &str) -> Option<Vec<&'static str>> {
    let normalized = normalize_rule_selector(selector);

    if let Some((_, group)) = RULE_GROUPS
        .iter()
        .find(|(name, _)| *name == normalized.as_str())
    {
        return Some((*group).to_vec());
    }

    canonical_rule_name(&normalized).map(|name| vec![name])
}

/// The selector closest to `selector`, when it is close enough to be a typo.
pub fn suggest_rule_selector(selector: &str) -> Option<&'static str> {
    let normalized = normalize_rule_selector(selector);

    ALL_RULE_NAMES
        .iter()
        .copied()
        .chain(RULE_GROUPS.iter().map(|(name, _)| *name))
        .map(|candidate| (edit_distance(&normalized, candidate), candidate))
        .filter(|(distance, _)| *distance <= 2)
        .min_by_key(|(distance, candidate)| (*distance, candidate.len()))
        .map(|(_, candidate)| candidate)
}

/// The number of single-character insertions, deletions and substitutions between `a` and
/// `b`.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];

    for (row, a_character) in a.chars().enumerate() {
        current[0] = row + 1;

        for (column, b_character) in b.iter().enumerate() {
            let substitution = previous[column] + usize::from(a_character != *b_character);
            current[column + 1] = substitution
                .min(previous[column + 1] + 1)
                .min(current[column] + 1);
        }

        std::mem::swap(&mut previous, &mut current);
    }

    previous[b.len()]
}

/// A `-r` value that names neither a rule nor a group.
#[derive(Debug)]
pub struct UnknownRule {
    selector: String,
    suggestion: Option<&'static str>,
}

impl std::fmt::Display for UnknownRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown rule `{}`", self.selector)?;

        if let Some(suggestion) = self.suggestion {
            write!(f, "\n  hint: did you mean `{suggestion}`?")?;
        }

        write!(
            f,
            "\n  note: `-r` takes comma-separated rule names, e.g. `-r cargo,nodejs`; \
             `ocy --rules` lists them all, and `-r caches` selects the shared caches"
        )
    }
}

impl std::error::Error for UnknownRule {}

/// The rules a scan actually uses.
///
/// With no selector the default set is every rule except the gated ones, and the flags add
/// their group to it. A selector replaces the default set entirely -- `-r` has always
/// meant "only these" -- and naming a gated rule is itself the opt-in, so
/// `-r cargo-registry` works without `--caches`. A selection is the narrowest thing a user
/// can express, and demanding a second flag to permit it would only be a way to forget
/// one.
pub fn select_rules(
    all: Vec<Rule>,
    selectors: &[String],
    include_caches: bool,
    include_commands: bool,
) -> Result<Vec<Rule>, UnknownRule> {
    if selectors.is_empty() {
        return Ok(all
            .into_iter()
            .filter(|rule| {
                (include_commands || !is_command_rule(&rule.name))
                    && (include_caches || !is_cache_rule(&rule.name))
            })
            .collect());
    }

    let mut selected = HashSet::new();
    for selector in selectors {
        selected.extend(expand_rule_selector(selector).ok_or_else(|| UnknownRule {
            selector: selector.clone(),
            suggestion: suggest_rule_selector(selector),
        })?);
    }

    Ok(all
        .into_iter()
        .filter(|rule| selected.contains(&*rule.name))
        .collect())
}

/// Hidden application state under the user's home, kept out of every rule regardless of
/// `--all`.
///
/// Hidden home directories hold installed applications' own settings and data, not build
/// output. An app that happens to bundle a
/// `package.json` beside a `node_modules` -- Discord's own native modules do -- looks
/// exactly like a project to the ordinary rules once `--all` lets the walk reach it, and
/// reclaiming its `node_modules` breaks the installed program rather than freeing
/// anything disposable.
///
/// Cache roots are deliberately excluded here because their contents are handled by the
/// explicit `--caches` rules. This makes unknown application directories safe without
/// requiring a growing list of tool names.
///
/// A location that fails to canonicalize -- nothing has created it yet -- is simply
/// left out: there is nothing under it to protect.
///
/// This protects a broader scan. A scan rooted *inside* one of these directories was
/// pointed there by name, and the walk drops ignores covering its root, so the requested
/// tree is scanned instead of being pruned to nothing -- see
/// [`ocy_core::walker::WalkOptions::ignores_covering`].
pub fn protected_state_dirs() -> Vec<PathBuf> {
    let Some(home) = home_directory() else {
        log::warn!("cannot locate the home directory, so no XDG state is protected");
        return Vec::new();
    };

    let cache_roots = [
        home.join(".cache"),
        env_dir("XDG_CACHE_HOME").unwrap_or_else(|| home.join(".cache")),
        env_dir("CARGO_HOME").unwrap_or_else(|| home.join(".cargo")),
        env_dir("GRADLE_USER_HOME").unwrap_or_else(|| home.join(".gradle")),
        home.join(".local"),
        home.join(".npm"),
        home.join(".bun"),
    ];

    let mut protected = std::fs::read_dir(&home)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('.'))
        })
        .filter(|path| !cache_roots.iter().any(|root| path == root))
        .filter_map(|path| path.canonicalize().ok())
        .collect::<Vec<_>>();

    protected.extend(
        [
            env_dir("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")),
            env_dir("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share")),
            env_dir("XDG_STATE_HOME").unwrap_or_else(|| home.join(".local/state")),
        ]
        .into_iter()
        .filter_map(|path| path.canonicalize().ok()),
    );
    protected
}

/// The hidden directories a scan with these rules needs to enter.
///
/// A cache anchor lives behind a leading dot -- `~/.cargo`, `~/.cache` -- so a rule
/// naming one would find nothing unless the walk were told to go in, and `--caches`
/// would appear to do nothing without `--all` beside it. Reading the names off the
/// anchors keeps that working when the environment has moved a cache somewhere else
/// hidden, and adds nothing at all when no cache rule is enabled.
pub fn scanned_hidden(rules: &[Rule]) -> HashSet<String> {
    SCANNED_HIDDEN_DIRS
        .iter()
        .map(|dir| (*dir).to_string())
        .chain(
            rules
                .iter()
                .filter_map(Rule::anchor)
                .flat_map(hidden_components),
        )
        .collect()
}

/// The dotted components of a path, which are the ones the walk has to be told about.
fn hidden_components(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .filter(|name| name.starts_with('.'))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests;
