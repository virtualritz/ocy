use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ocy_core::rule::{Rule, RuleError};

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

/// The built-in rule set.
///
/// Command rules are gated behind `allow_commands`: a rule such as `make clean` runs a
/// script the scanned directory controls, so enabling it for an ordinary scan would
/// execute arbitrary code from any tree that happens to contain a `Makefile`.
///
/// Shared tool caches are gated behind `clean_caches`, for a different reason: they are
/// not project output. One belongs to every project on the machine at once, so a scan of
/// one directory reclaiming it would reach far outside what was asked for.
pub fn standard_rules(allow_commands: bool, clean_caches: bool) -> Result<Vec<Rule>, RuleError> {
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
        Rule::remove("Cargo", &["Cargo.toml"], &["target"])?,
        Rule::remove_self("Cargo", &["CACHEDIR.TAG", ".rustc_info.json"])?,
        // JVM. The glob covers both the Groovy and Kotlin build scripts.
        Rule::remove("Gradle", &["build.gradle*"], &["build", ".gradle"])?,
        Rule::remove("Maven", &["pom.xml"], &["target"])?,
        Rule::remove("SBT", &["build.sbt"], &["target", "project/target"])?,
        // JavaScript and TypeScript.
        Rule::remove(
            "NodeJS",
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
        Rule::remove("Angular", &["angular.json"], &[".angular/cache"])?,
        // Python. A virtual environment is identified by the `pyvenv.cfg` it contains,
        // which is why it needs a self-marked rule rather than a sibling one.
        Rule::remove_self("Python venv", &["pyvenv.cfg"])?,
        Rule::remove("Python", &["*.py"], &["__pycache__"])?,
        Rule::remove("Python", &["pyproject.toml"], PYTHON_CACHES)?,
        Rule::remove("Python", &["setup.py"], PYTHON_CACHES)?,
        // .NET.
        Rule::remove(".NET", &["*.csproj"], DOTNET_OUTPUT)?,
        Rule::remove(".NET", &["*.fsproj"], DOTNET_OUTPUT)?,
        // C and C++. A CMake build directory may be called anything -- `build`,
        // `build_debug`, `Linux-x86_64-optimize` -- and is as often a sibling of the
        // source as a child of it, so the sibling rule alone misses most of them.
        // `CMakeCache.txt` is written into the build directory itself, which makes the
        // directory self-marked whatever it was named.
        Rule::remove(
            "CMake",
            &["CMakeLists.txt"],
            &["cmake-build-*", "CMakeFiles"],
        )?,
        Rule::remove_self("CMake", &["CMakeCache.txt"])?,
        // Zig.
        Rule::remove(
            "Zig",
            &["build.zig"],
            &[".zig-cache", "zig-cache", "zig-out"],
        )?,
        // Swift and Objective-C.
        Rule::remove("SwiftPM", &["Package.swift"], &[".build"])?,
        Rule::remove("XCode", &["*.xcodeproj"], &["DerivedData"])?,
        // Elixir.
        Rule::remove("Elixir", &["mix.exs"], &["_build", "deps"])?,
        // Haskell.
        Rule::remove("Cabal", &["*.cabal"], &["dist-newstyle"])?,
        Rule::remove("Stack", &["stack.yaml"], &[".stack-work"])?,
        // Dart and Flutter.
        Rule::remove("Flutter/Dart", &["pubspec.yaml"], &["build", ".dart_tool"])?,
        // Unity, which needs both markers to avoid matching any directory named `Assets`.
        Rule::remove(
            "Unity",
            &["Assets", "ProjectSettings"],
            &["Library", "Temp", "Obj", "Logs"],
        )?,
        // WebAssembly. Keyed on `Trunk.toml` rather than on the manifest beside it: a
        // `dist` next to a `Cargo.toml` is as likely to be something the project keeps.
        Rule::remove("Trunk", &["Trunk.toml"], &["dist"])?,
        // Infrastructure.
        Rule::remove("Terraform", &["*.tf"], &[".terraform"])?,
        // PHP.
        Rule::remove("Composer", &["composer.json"], &["vendor"])?,
        // Git worktree records left behind by a deleted checkout.
        Rule::prune_stale_worktrees("Git worktree", &[".git"])?,
    ];

    let command_rules = allow_commands
        .then(|| Rule::run("Make", &["Makefile"], "make clean"))
        .transpose()?
        .into_iter();

    let cache_rules = if clean_caches {
        cache_rules()?
    } else {
        Vec::new()
    };

    Ok(rules
        .into_iter()
        .chain(command_rules)
        .chain(cache_rules)
        .collect())
}

/// Entries under the cache home that exist only to be a cache.
///
/// Every one of these is refetched or rebuilt on demand, so losing it costs time and
/// nothing else. `~/.cache` also holds state that is not reproducible, which is why the
/// list is named out rather than inferred from the shape of what is in there.
const TOOL_CACHES: &[&str] = &[
    // Compiler caches.
    "sccache",
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
    // Python tooling.
    "pip",
    "uv",
    // Go tooling.
    "go-build",
];

/// Rules for the caches that tools keep once for the whole machine.
///
/// These are anchored to a path rather than matched by markers, because that is what
/// identifies them: a cache is a cache by virtue of where the tool agreed to put it, and
/// nothing inside `~/.cache/sccache` says so. Anchoring also means the rule cannot fire
/// against a project directory that happens to look similar.
///
/// Each location can be moved by an environment variable, and a scan that only knew the
/// default would quietly find nothing on a machine that had moved it.
fn cache_rules() -> Result<Vec<Rule>, RuleError> {
    let Some(home) = home_directory() else {
        log::warn!("cannot locate the home directory, so no shared cache is known");
        return Ok(Vec::new());
    };

    let cargo = env_dir("CARGO_HOME").unwrap_or_else(|| home.join(".cargo"));
    let gradle = env_dir("GRADLE_USER_HOME").unwrap_or_else(|| home.join(".gradle"));
    let cache = env_dir("XDG_CACHE_HOME").unwrap_or_else(|| home.join(".cache"));

    Ok(vec![
        // Downloaded crates and the sources unpacked from them. `index` is left alone:
        // it is small, and refetching it stalls the next build of every project at once.
        Rule::remove_at("Cargo registry", cargo.join("registry"), &["cache", "src"])?,
        // Bare clones of git dependencies and the working copies checked out from them.
        Rule::remove_at("Cargo git", cargo.join("git"), &["checkouts", "db"])?,
        // Gradle's own `wrapper` directory is left alone: it holds the Gradle
        // distributions themselves, which is an installation rather than a cache.
        Rule::remove_at("Gradle cache", gradle, &["caches", "daemon"])?,
        Rule::remove_at("Tool cache", cache, TOOL_CACHES)?,
    ])
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
