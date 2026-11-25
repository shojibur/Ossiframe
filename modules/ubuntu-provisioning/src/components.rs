//! Component data and dependency resolution. Data only: which packages,
//! units and limits belong to which component. Ubuntu 24.04 (noble) archive
//! packages only. No PPA, no network-fetched script.
//!
//! Package names were checked for existence in the noble archive on
//! packages.ubuntu.com (pages `https://packages.ubuntu.com/noble/<package>`):
//!
//! - `php8.3-cli`, `-common`, `-opcache`, `-mbstring`, `-xml`, `-curl`, `-zip`,
//!   `-bcmath`, `-intl`, `-mysql`, `-sqlite3`, `-gd`, `-fpm`: each page exists
//!   for noble (source package `php8.3`, version `8.3.6-0ubuntu0.24.04.13`,
//!   see `https://packages.ubuntu.com/source/noble/php8.3`).
//! - `composer` (2.7.1-2), `git` (1:2.43.0-1ubuntu7.4), `nginx`
//!   (1.24.0-2ubuntu7.19), `mysql-server` (8.0.46-0ubuntu0.24.04.4),
//!   `redis-server` (5:7.0.15-1ubuntu0.24.04.5): each page exists for noble.
//!
//! Not confirmed by this check (assumptions for live verification, S04
//! task 8): the archive component (main or universe) of each package and
//! therefore whether it is visible with default sources on every image;
//! `php8.3-cli` pulling `php8.3-common` and `php8.3-readline` as
//! dependencies; that `php8.3-fpm` creates `/run/php/php8.3-fpm.sock`; and
//! that the versions above are what `apt-cache policy` shows on the target
//! host. The versions are listed here as evidence of the check only. They
//! are not claims about any server and must not be shown as such.

use ossiframe_core::{ComponentInfo, PlanError};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComponentDef {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub packages: &'static [&'static str],
    pub requires: &'static [&'static str],
    /// systemd units expected to be active after installation.
    pub units: &'static [&'static str],
    pub est_disk_mb: u32,
    pub min_mem_mb: u32,
    /// Packages whose presence conflicts with this component.
    pub blocks_on_packages: &'static [&'static str],
    pub optional: bool,
}

/// Definition order is also the resolution order. Every component appears
/// after the components it requires (checked by a unit test).
pub const COMPONENTS: &[ComponentDef] = &[
    ComponentDef {
        id: "git",
        title: "Git",
        description: "Distributed version control",
        packages: &["git"],
        requires: &[],
        units: &[],
        est_disk_mb: 60,
        min_mem_mb: 0,
        blocks_on_packages: &[],
        optional: false,
    },
    ComponentDef {
        id: "nginx",
        title: "NGINX",
        description: "Web server with the distribution default site",
        packages: &["nginx"],
        requires: &[],
        units: &["nginx"],
        est_disk_mb: 50,
        min_mem_mb: 0,
        blocks_on_packages: &["apache2"],
        optional: false,
    },
    ComponentDef {
        id: "php",
        title: "PHP 8.3 CLI",
        description: "PHP 8.3 command line and the extensions Laravel needs",
        packages: &[
            "php8.3-bcmath",
            "php8.3-cli",
            "php8.3-common",
            "php8.3-curl",
            "php8.3-gd",
            "php8.3-intl",
            "php8.3-mbstring",
            "php8.3-mysql",
            "php8.3-opcache",
            "php8.3-sqlite3",
            "php8.3-xml",
            "php8.3-zip",
        ],
        requires: &[],
        units: &[],
        est_disk_mb: 150,
        min_mem_mb: 0,
        blocks_on_packages: &[],
        optional: false,
    },
    ComponentDef {
        id: "php-fpm",
        title: "PHP-FPM 8.3",
        description: "FastCGI process manager with the default pool",
        packages: &["php8.3-fpm"],
        requires: &["php"],
        units: &["php8.3-fpm"],
        est_disk_mb: 20,
        min_mem_mb: 0,
        blocks_on_packages: &[],
        optional: false,
    },
    ComponentDef {
        id: "composer",
        title: "Composer",
        description: "PHP dependency manager from the Ubuntu archive",
        packages: &["composer"],
        requires: &["php"],
        units: &[],
        est_disk_mb: 30,
        min_mem_mb: 0,
        blocks_on_packages: &[],
        optional: false,
    },
    ComponentDef {
        id: "mysql",
        title: "MySQL",
        description: "MySQL server from the Ubuntu archive",
        packages: &["mysql-server"],
        requires: &[],
        units: &["mysql"],
        est_disk_mb: 500,
        min_mem_mb: 900,
        blocks_on_packages: &["mariadb-server"],
        optional: false,
    },
    ComponentDef {
        id: "redis",
        title: "Redis",
        description: "In-memory data store",
        packages: &["redis-server"],
        requires: &[],
        units: &["redis-server"],
        est_disk_mb: 20,
        min_mem_mb: 0,
        blocks_on_packages: &[],
        optional: true,
    },
];

/// Selected when the user changes nothing. `redis` is optional and unselected.
pub const DEFAULT_SELECTION: &[&str] = &["git", "nginx", "php", "php-fpm", "composer", "mysql"];

pub fn find(id: &str) -> Option<&'static ComponentDef> {
    COMPONENTS.iter().find(|c| c.id == id)
}

pub fn infos() -> Vec<ComponentInfo> {
    COMPONENTS
        .iter()
        .map(|c| ComponentInfo {
            id: c.id.to_owned(),
            title: c.title.to_owned(),
            description: c.description.to_owned(),
            requires: c.requires.iter().map(|r| (*r).to_owned()).collect(),
            optional: c.optional,
        })
        .collect()
}

/// Returns the selection plus everything it requires, dependencies first.
/// The result depends only on the set of ids, not on their order or repeats.
/// Any subset is allowed. An unknown id is an error.
pub fn resolve(selection: &[String]) -> Result<Vec<&'static ComponentDef>, PlanError> {
    let mut wanted: BTreeSet<&'static str> = BTreeSet::new();
    let mut todo: Vec<&'static ComponentDef> = Vec::new();
    for id in selection {
        let def = find(id).ok_or_else(|| PlanError::UnknownComponent(id.clone()))?;
        todo.push(def);
    }
    while let Some(def) = todo.pop() {
        if wanted.insert(def.id) {
            for r in def.requires {
                // Requirements are static data; a miss is a programming error
                // that the data test catches.
                let dep = find(r).ok_or_else(|| PlanError::UnknownComponent((*r).to_owned()))?;
                todo.push(dep);
            }
        }
    }
    Ok(COMPONENTS
        .iter()
        .filter(|c| wanted.contains(c.id))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| (*s).to_owned()).collect()
    }

    fn ids(ids: &[&str]) -> Vec<&'static str> {
        resolve(&sel(ids)).unwrap().iter().map(|c| c.id).collect()
    }

    #[test]
    fn data_is_consistent() {
        for (i, c) in COMPONENTS.iter().enumerate() {
            assert!(!c.packages.is_empty(), "{}", c.id);
            for r in c.requires {
                let at = COMPONENTS.iter().position(|d| d.id == *r);
                assert!(at.is_some_and(|p| p < i), "{} requires {r} later", c.id);
            }
            assert!(c.packages.iter().all(|p| {
                !p.is_empty()
                    && p.bytes().all(|b| {
                        b.is_ascii_lowercase() || b.is_ascii_digit() || b"+-.".contains(&b)
                    })
            }));
        }
        let all: Vec<_> = COMPONENTS.iter().map(|c| c.id).collect();
        let unique: BTreeSet<_> = all.iter().collect();
        assert_eq!(all.len(), unique.len());
    }

    #[test]
    fn php_fpm_pulls_php_first() {
        assert_eq!(ids(&["php-fpm"]), ["php", "php-fpm"]);
        assert_eq!(
            ids(&["composer", "php-fpm"]),
            ["php", "php-fpm", "composer"]
        );
    }

    #[test]
    fn any_subset_resolves() {
        assert_eq!(ids(&["nginx"]), ["nginx"]);
        assert_eq!(ids(&["php"]), ["php"]);
        assert_eq!(ids(&["php", "nginx", "git"]), ["git", "nginx", "php"]);
        assert!(ids(&[]).is_empty());
    }

    #[test]
    fn resolution_is_deterministic_and_order_independent() {
        let a = ids(&["mysql", "php-fpm", "git", "php-fpm"]);
        assert_eq!(a, ids(&["git", "php-fpm", "mysql"]));
        assert_eq!(a, ids(&["mysql", "php-fpm", "git", "php-fpm"]));
    }

    #[test]
    fn unknown_id_is_an_error() {
        assert_eq!(
            resolve(&sel(&["nginx", "laravel"])).unwrap_err(),
            PlanError::UnknownComponent("laravel".into())
        );
    }

    #[test]
    fn default_selection_excludes_redis_and_resolves_fully() {
        assert!(!DEFAULT_SELECTION.contains(&"redis"));
        let resolved = resolve(&sel(DEFAULT_SELECTION)).unwrap();
        assert_eq!(resolved.len(), DEFAULT_SELECTION.len());
        let optional: Vec<_> = COMPONENTS.iter().filter(|c| c.optional).collect();
        assert_eq!(optional.len(), 1);
        assert_eq!(optional[0].id, "redis");
    }

    #[test]
    fn components_carry_no_runtime_typed_server_profile() {
        // Components are independent capabilities; none requires a "laravel"
        // or application-level component.
        for c in COMPONENTS {
            assert!(c.requires.iter().all(|r| find(r).is_some()));
            assert!(!c.id.contains("laravel"));
        }
    }
}
