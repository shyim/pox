//! Dependency graph analysis for installed packages.

use std::collections::HashSet;
use std::sync::Arc;

use crate::package::{Link, LinkType, Package};
use pox_semver::ConstraintInterface;

#[derive(Debug, Clone)]
pub struct DependencyResult {
    pub package: Arc<Package>,
    pub link: Link,
    pub children: Option<Vec<DependencyResult>>,
}

pub fn get_dependents(
    packages: &[Arc<Package>],
    needles: &[String],
    constraint: Option<&dyn ConstraintInterface>,
    invert: bool,
    recurse: bool,
    packages_found: Option<HashSet<String>>,
) -> Vec<DependencyResult> {
    let needles: Vec<String> = needles.iter().map(|n| n.to_lowercase()).collect();
    let mut results = Vec::new();

    let packages_found = packages_found.unwrap_or_else(|| {
        needles.iter().cloned().collect()
    });

    let _root_package = packages.iter().find(|p| {
        p.package_type.as_str() == "project" ||
        p.package_type.as_str() == "root-package"
    });

    for package in packages {
        let mut links = hashmap_to_links(&package.require, &package.name, LinkType::Require);
        let mut packages_in_tree = packages_found.clone();

        if !invert {
            let replace_links = hashmap_to_links(&package.replace, &package.name, LinkType::Replace);
            links.extend(replace_links.clone());

            for replace_link in &replace_links {
                for needle in &needles {
                    if package.name.to_lowercase() == *needle {
                        if constraint.is_none() || matches_constraint(&replace_link.constraint, constraint, false) {
                            let target_lower = replace_link.target.to_lowercase();
                            if packages_in_tree.contains(&target_lower) {
                                results.push(DependencyResult {
                                    package: package.clone(),
                                    link: replace_link.clone(),
                                    children: None,
                                });
                                continue;
                            }
                            packages_in_tree.insert(target_lower.clone());

                            let dependents = if recurse {
                                get_dependents(
                                    packages,
                                    &[target_lower.clone()],
                                    None,
                                    false,
                                    true,
                                    Some(packages_in_tree.clone()),
                                )
                            } else {
                                Vec::new()
                            };

                            results.push(DependencyResult {
                                package: package.clone(),
                                link: replace_link.clone(),
                                children: Some(dependents),
                            });
                        }
                    }
                }
            }
        }

        if package.package_type.as_str() == "project" ||
           package.package_type.as_str() == "root-package" {
            links.extend(hashmap_to_links(&package.require_dev, &package.name, LinkType::DevRequire));
        }

        for link in &links {
            for needle in &needles {
                if link.target.to_lowercase() == *needle {
                    if constraint.is_none() || matches_constraint(&link.constraint, constraint, invert) {
                        let source_lower = package.name.to_lowercase();
                        if packages_in_tree.contains(&source_lower) {
                            results.push(DependencyResult {
                                package: package.clone(),
                                link: link.clone(),
                                children: None,
                            });
                            continue;
                        }
                        packages_in_tree.insert(source_lower.clone());

                        let dependents = if recurse {
                            get_dependents(
                                packages,
                                &[source_lower.clone()],
                                None,
                                false,
                                true,
                                Some(packages_in_tree.clone()),
                            )
                        } else {
                            Vec::new()
                        };

                        results.push(DependencyResult {
                            package: package.clone(),
                            link: link.clone(),
                            children: Some(dependents),
                        });
                    }
                }
            }
        }

        if invert && needles.contains(&package.name.to_lowercase()) {
            let conflict_links = hashmap_to_links(&package.conflict, &package.name, LinkType::Conflict);
            for conflict_link in &conflict_links {
                for other_pkg in packages {
                    if other_pkg.name.to_lowercase() == conflict_link.target.to_lowercase() {
                        if constraint_matches_version(&conflict_link.constraint, &other_pkg.version) == invert {
                            results.push(DependencyResult {
                                package: package.clone(),
                                link: conflict_link.clone(),
                                children: None,
                            });
                        }
                    }
                }
            }
        }

        let conflict_links = hashmap_to_links(&package.conflict, &package.name, LinkType::Conflict);
        for conflict_link in &conflict_links {
            if needles.contains(&conflict_link.target.to_lowercase()) {
                for other_pkg in packages {
                    if other_pkg.name.to_lowercase() == conflict_link.target.to_lowercase() {
                        if constraint_matches_version(&conflict_link.constraint, &other_pkg.version) == invert {
                            results.push(DependencyResult {
                                package: package.clone(),
                                link: conflict_link.clone(),
                                children: None,
                            });
                        }
                    }
                }
            }
        }
    }

    results
}

pub fn find_packages_with_replacers_and_providers(
    packages: &[Arc<Package>],
    name: &str,
    constraint: Option<&dyn ConstraintInterface>,
) -> Vec<Arc<Package>> {
    let name_lower = name.to_lowercase();
    let mut matches = Vec::new();

    for package in packages {
        if package.name.to_lowercase() == name_lower {
            if constraint.is_none() || constraint_matches_version(&constraint.unwrap().to_string(), &package.version) {
                matches.push(package.clone());
            }
            continue;
        }

        for (target, target_constraint) in package.provide.iter().chain(package.replace.iter()) {
            if target.to_lowercase() == name_lower {
                if constraint.is_none() || matches_constraint(target_constraint, constraint, false) {
                    matches.push(package.clone());
                    break;
                }
            }
        }
    }

    matches
}

fn matches_constraint(link_constraint: &str, filter_constraint: Option<&dyn ConstraintInterface>, invert: bool) -> bool {
    if let Some(filter) = filter_constraint {
        let parser = pox_semver::VersionParser;
        if let Ok(parsed) = parser.parse_constraints(link_constraint) {
            let matches = parsed.to_string() == filter.to_string();
            matches != invert
        } else {
            !invert
        }
    } else {
        true
    }
}

fn constraint_matches_version(constraint_str: &str, version: &str) -> bool {
    let parser = pox_semver::VersionParser;
    match parser.parse_constraints(constraint_str) {
        Ok(constraint) => {
            match parser.parse_constraints(version) {
                Ok(ver_constraint) => {
                    constraint.to_string() == ver_constraint.to_string()
                },
                Err(_) => false,
            }
        },
        Err(_) => false,
    }
}

fn hashmap_to_links(
    map: &indexmap::IndexMap<String, String>,
    source: &str,
    link_type: LinkType,
) -> Vec<Link> {
    map.iter()
        .map(|(target, constraint)| Link {
            source: source.to_string(),
            target: target.clone(),
            constraint: constraint.clone(),
            pretty_constraint: Some(constraint.clone()),
            link_type,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    //! Tests ported from Composer's BaseDependencyCommandTest.php

    use super::*;

    fn pkg(name: &str, version: &str) -> Package {
        Package::new(name, version)
    }

    fn pkg_with_requires(name: &str, version: &str, requires: Vec<(&str, &str)>) -> Package {
        let mut p = Package::new(name, version);
        for (dep_name, constraint) in requires {
            p.require.insert(dep_name.to_string(), constraint.to_string());
        }
        p
    }

    fn pkg_with_require_dev(name: &str, version: &str, require_dev: Vec<(&str, &str)>) -> Package {
        let mut p = Package::new(name, version);
        p.package_type = "root-package".to_string();
        for (dep_name, constraint) in require_dev {
            p.require_dev.insert(dep_name.to_string(), constraint.to_string());
        }
        p
    }

    fn pkg_with_replaces(name: &str, version: &str, replaces: Vec<(&str, &str)>) -> Package {
        let mut p = Package::new(name, version);
        for (replace_name, constraint) in replaces {
            p.replace.insert(replace_name.to_string(), constraint.to_string());
        }
        p
    }

    fn pkg_with_provides(name: &str, version: &str, provides: Vec<(&str, &str)>) -> Package {
        let mut p = Package::new(name, version);
        for (provide_name, constraint) in provides {
            p.provide.insert(provide_name.to_string(), constraint.to_string());
        }
        p
    }

    #[test]
    fn test_find_direct_dependent() {
        let pkg1 = Arc::new(pkg_with_requires("vendor/package1", "1.0.0", vec![("vendor/dependency", "^1.0")]));
        let pkg2 = Arc::new(pkg("vendor/dependency", "1.0.0"));

        let packages = vec![pkg1.clone(), pkg2.clone()];
        let results = get_dependents(&packages, &["vendor/dependency".to_string()], None, false, false, None);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].package.name, "vendor/package1");
        assert_eq!(results[0].link.target, "vendor/dependency");
    }

    #[test]
    fn test_no_dependents() {
        let pkg1 = Arc::new(pkg("vendor/package1", "1.0.0"));
        let pkg2 = Arc::new(pkg("vendor/package2", "1.0.0"));

        let packages = vec![pkg1.clone(), pkg2.clone()];
        let results = get_dependents(&packages, &["vendor/package1".to_string()], None, false, false, None);

        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_nested_dependencies() {
        let pkg1 = Arc::new(pkg_with_requires("vendor/package1", "1.3.0", vec![("vendor/package2", "^2")]));
        let pkg2 = Arc::new(pkg_with_requires("vendor/package2", "2.3.0", vec![("vendor/package3", "^1")]));
        let pkg3 = Arc::new(pkg("vendor/package3", "2.1.0"));
        let root = Arc::new(pkg_with_requires("__root__", "dev-main", vec![("vendor/package2", "1.3.0"), ("vendor/package3", "2.3.0")]));
        root.as_ref().clone().package_type = "root-package".to_string();

        let packages = vec![root, pkg1, pkg2, pkg3];
        let results = get_dependents(&packages, &["vendor/package3".to_string()], None, false, false, None);

        assert_eq!(results.len(), 2);
        let names: Vec<&str> = results.iter().map(|r| r.package.name.as_str()).collect();
        assert!(names.contains(&"__root__"));
        assert!(names.contains(&"vendor/package2"));
    }

    #[test]
    fn test_recursive_dependencies() {
        let pkg1 = Arc::new(pkg_with_requires("vendor/package1", "1.3.0", vec![("vendor/package2", "^2")]));
        let pkg2 = Arc::new(pkg_with_requires("vendor/package2", "2.3.0", vec![("vendor/package3", "^1")]));
        let pkg3 = Arc::new(pkg("vendor/package3", "2.1.0"));
        let mut root = pkg_with_requires("__root__", "dev-main", vec![("vendor/package2", "1.3.0"), ("vendor/package3", "2.3.0")]);
        root.package_type = "root-package".to_string();

        let packages = vec![Arc::new(root), pkg1.clone(), pkg2, pkg3];
        let results = get_dependents(&packages, &["vendor/package3".to_string()], None, false, true, None);

        assert!(results.len() >= 2);
        let has_nested = results.iter().any(|r| r.children.is_some() && !r.children.as_ref().unwrap().is_empty());
        assert!(has_nested, "Should have recursive dependencies");
    }

    #[test]
    fn test_dev_dependency() {
        let pkg1 = Arc::new(pkg("vendor/package1", "2.0.0"));
        let mut root = pkg_with_require_dev("__root__", "dev-main", vec![("vendor/package1", "2.*")]);
        root.package_type = "root-package".to_string();

        let packages = vec![Arc::new(root), pkg1];
        let results = get_dependents(&packages, &["vendor/package1".to_string()], None, false, false, None);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].package.name, "__root__");
        assert_eq!(results[0].link.link_type, LinkType::DevRequire);
    }

    #[test]
    fn test_find_with_replacers() {
        let pkg1 = Arc::new(pkg_with_replaces("vendor/package1", "1.0.0", vec![("vendor/old-package", "*")]));

        let packages = vec![pkg1.clone()];
        let results = find_packages_with_replacers_and_providers(&packages, "vendor/old-package", None);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "vendor/package1");
    }

    #[test]
    fn test_find_with_providers() {
        let pkg1 = Arc::new(pkg_with_provides("vendor/package1", "1.0.0", vec![("vendor/virtual-package", "^1.0")]));

        let packages = vec![pkg1.clone()];
        let results = find_packages_with_replacers_and_providers(&packages, "vendor/virtual-package", None);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "vendor/package1");
    }

    #[test]
    fn test_circular_dependency_detection() {
        let pkg1 = Arc::new(pkg_with_requires("vendor/package1", "1.0.0", vec![("vendor/package2", "^1.0")]));
        let pkg2 = Arc::new(pkg_with_requires("vendor/package2", "1.0.0", vec![("vendor/package3", "^1.0")]));
        let pkg3 = Arc::new(pkg_with_requires("vendor/package3", "1.0.0", vec![("vendor/package2", "^1.0")]));

        let packages = vec![pkg1, pkg2, pkg3];
        let results = get_dependents(&packages, &["vendor/package2".to_string()], None, false, true, None);

        let has_circular = results.iter().any(|r| {
            if let Some(ref children) = r.children {
                children.iter().any(|c| c.children.is_none())
            } else {
                false
            }
        });
        assert!(has_circular, "Should detect circular dependency in nested results");
    }

    #[test]
    fn test_case_insensitive_matching() {
        let pkg1 = Arc::new(pkg_with_requires("Vendor/Package1", "1.0.0", vec![("Vendor/Dependency", "^1.0")]));
        let pkg2 = Arc::new(pkg("vendor/dependency", "1.0.0"));

        let packages = vec![pkg1.clone(), pkg2.clone()];
        let results = get_dependents(&packages, &["VENDOR/DEPENDENCY".to_string()], None, false, false, None);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].package.name.to_lowercase(), "vendor/package1");
        assert_eq!(results[0].link.target.to_lowercase(), "vendor/dependency");
    }
}
