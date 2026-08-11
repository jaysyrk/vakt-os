//! Working out what has to be installed, and in what order.
//!
//! Dependencies form a directed graph, so installing in a safe order is a
//! topological sort - defined only if the graph is acyclic, which is why a
//! cycle is reported by naming the packages in the loop rather than recursing
//! until the stack runs out.
//!
//! Kept separate from fetching: everything here works on an already-collected
//! map of manifests, so the ordering logic is pure and testable offline.

use crate::manifest::PackageManifest;
use anyhow::{Result, bail};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Where a node is in the depth-first walk. The `InProgress` state is what
/// distinguishes a cycle from a package that legitimately appears twice in the
/// graph because two things depend on it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Visit {
    InProgress,
    Done,
}

/// Returns the packages to install, dependencies before dependents.
///
/// `roots` are the packages the user asked for. `manifests` must already
/// contain every package reachable from them; a missing one is an error naming
/// both the absent package and what wanted it, because "not found" on its own
/// is useless when the request was three levels deep.
pub fn install_order(
    roots: &[String],
    manifests: &BTreeMap<String, PackageManifest>,
) -> Result<Vec<String>> {
    let mut state: HashMap<&str, Visit> = HashMap::new();
    let mut order: Vec<String> = Vec::new();

    for root in roots {
        visit(
            root,
            None,
            manifests,
            &mut state,
            &mut order,
            &mut Vec::new(),
        )?;
    }
    Ok(order)
}

fn visit<'a>(
    name: &'a str,
    wanted_by: Option<&str>,
    manifests: &'a BTreeMap<String, PackageManifest>,
    state: &mut HashMap<&'a str, Visit>,
    order: &mut Vec<String>,
    path: &mut Vec<&'a str>,
) -> Result<()> {
    match state.get(name) {
        Some(Visit::Done) => return Ok(()),
        Some(Visit::InProgress) => {
            // `path` is the chain of packages currently being visited, so the
            // cycle is its tail from wherever this name first appeared.
            let start = path.iter().position(|p| *p == name).unwrap_or(0);
            let mut cycle: Vec<&str> = path[start..].to_vec();
            cycle.push(name);
            bail!("Dependency cycle: {}", cycle.join(" -> "));
        }
        None => {}
    }

    let Some(manifest) = manifests.get(name) else {
        match wanted_by {
            Some(parent) => bail!(
                "Package '{}' depends on '{}', which the repository does not have",
                parent,
                name
            ),
            None => bail!("Package '{}' is not in the repository", name),
        }
    };

    state.insert(&manifest.name, Visit::InProgress);
    path.push(&manifest.name);

    for dependency in &manifest.dependencies {
        visit(
            dependency,
            Some(&manifest.name),
            manifests,
            state,
            order,
            path,
        )?;
    }

    path.pop();
    state.insert(&manifest.name, Visit::Done);
    order.push(manifest.name.clone());
    Ok(())
}

/// Every package name reachable from `roots`, for the fetcher to collect.
///
/// Used to walk the graph one level at a time while manifests are still being
/// downloaded, so the fetch terminates even if the repository publishes a
/// cycle - the cycle is then reported by [`install_order`] with a proper
/// message instead of by an infinite download loop.
pub fn undiscovered(
    roots: &[String],
    manifests: &BTreeMap<String, PackageManifest>,
) -> Vec<String> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut missing: Vec<String> = Vec::new();

    let mut queue: Vec<&str> = roots.iter().map(String::as_str).collect();
    while let Some(name) = queue.pop() {
        if !seen.insert(name) {
            continue;
        }
        match manifests.get(name) {
            Some(manifest) => queue.extend(manifest.dependencies.iter().map(String::as_str)),
            None => missing.push(name.to_string()),
        }
    }

    missing.sort();
    missing.dedup();
    missing
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(name: &str, dependencies: &[&str]) -> PackageManifest {
        PackageManifest {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            description: String::new(),
            dependencies: dependencies.iter().map(|s| s.to_string()).collect(),
            signature: String::new(),
        }
    }

    fn graph(entries: &[(&str, &[&str])]) -> BTreeMap<String, PackageManifest> {
        entries
            .iter()
            .map(|(name, deps)| (name.to_string(), manifest(name, deps)))
            .collect()
    }

    fn roots(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_package_with_no_dependencies_is_its_own_plan() {
        let graph = graph(&[("vakt-audit", &[])]);
        assert_eq!(
            install_order(&roots(&["vakt-audit"]), &graph).unwrap(),
            vec!["vakt-audit"]
        );
    }

    #[test]
    fn dependencies_are_installed_before_the_packages_that_need_them() {
        let graph = graph(&[
            ("vakt-audit", &["libvakt"][..]),
            ("libvakt", &["libcore"][..]),
            ("libcore", &[][..]),
        ]);

        assert_eq!(
            install_order(&roots(&["vakt-audit"]), &graph).unwrap(),
            vec!["libcore", "libvakt", "vakt-audit"]
        );
    }

    /// A package two things depend on is installed once, before both of them.
    #[test]
    fn a_shared_dependency_appears_exactly_once() {
        let graph = graph(&[
            ("vakt-audit", &["libvakt"][..]),
            ("vakt-ids", &["libvakt"][..]),
            ("libvakt", &[][..]),
        ]);

        let order = install_order(&roots(&["vakt-audit", "vakt-ids"]), &graph).unwrap();
        assert_eq!(order, vec!["libvakt", "vakt-audit", "vakt-ids"]);
        assert_eq!(order.iter().filter(|n| *n == "libvakt").count(), 1);
    }

    /// The ordering must hold no matter which order the user names things in.
    #[test]
    fn ordering_does_not_depend_on_the_order_of_the_request() {
        let graph = graph(&[
            ("top", &["middle"][..]),
            ("middle", &["bottom"][..]),
            ("bottom", &[][..]),
        ]);

        let forwards = install_order(&roots(&["top", "bottom"]), &graph).unwrap();
        let backwards = install_order(&roots(&["bottom", "top"]), &graph).unwrap();
        assert_eq!(forwards, vec!["bottom", "middle", "top"]);
        assert_eq!(backwards, vec!["bottom", "middle", "top"]);
    }

    #[test]
    fn a_cycle_is_reported_rather_than_followed() {
        let graph = graph(&[("a", &["b"][..]), ("b", &["c"][..]), ("c", &["a"][..])]);

        let error = install_order(&roots(&["a"]), &graph)
            .unwrap_err()
            .to_string();
        assert!(error.contains("Dependency cycle"), "got: {}", error);
        assert!(error.contains("a -> b -> c -> a"), "got: {}", error);
    }

    #[test]
    fn a_package_depending_on_itself_is_a_cycle() {
        let graph = graph(&[("a", &["a"][..])]);
        let error = install_order(&roots(&["a"]), &graph)
            .unwrap_err()
            .to_string();
        assert!(error.contains("Dependency cycle"), "got: {}", error);
    }

    #[test]
    fn a_missing_dependency_names_what_wanted_it() {
        let graph = graph(&[("vakt-audit", &["libvakt"][..])]);
        let error = install_order(&roots(&["vakt-audit"]), &graph)
            .unwrap_err()
            .to_string();
        assert!(error.contains("vakt-audit"), "got: {}", error);
        assert!(error.contains("libvakt"), "got: {}", error);
    }

    #[test]
    fn a_missing_root_says_so_plainly() {
        let error = install_order(&roots(&["ghost"]), &BTreeMap::new())
            .unwrap_err()
            .to_string();
        assert_eq!(error, "Package 'ghost' is not in the repository");
    }

    #[test]
    fn undiscovered_reports_what_still_has_to_be_fetched() {
        let partial = graph(&[("a", &["b", "c"][..])]);
        assert_eq!(undiscovered(&roots(&["a"]), &partial), vec!["b", "c"]);

        let complete = graph(&[("a", &["b"][..]), ("b", &[][..])]);
        assert!(undiscovered(&roots(&["a"]), &complete).is_empty());
    }

    /// The fetch walk has to terminate on a cyclic repository too, or the
    /// error would never get as far as being reported.
    #[test]
    fn undiscovered_terminates_on_a_cycle() {
        let graph = graph(&[("a", &["b"][..]), ("b", &["a"][..])]);
        assert!(undiscovered(&roots(&["a"]), &graph).is_empty());
    }
}
