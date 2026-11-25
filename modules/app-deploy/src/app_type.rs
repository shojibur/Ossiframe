//! Application types as data. Adding a type means adding a variant and a
//! row in [`app_types`]; no other crate changes.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AppType {
    Laravel,
    Php,
    Static,
}

/// What the type picker shows and what plan building needs to know.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppTypeInfo {
    pub app_type: AppType,
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// Served through a PHP-FPM pool.
    pub uses_php: bool,
    /// A MySQL database is created by default (PHP: opt-in, Static: never).
    pub database: DatabaseUse,
    /// The entry file is fixed (Laravel) or user-chosen with this default.
    pub entry_file: &'static str,
    pub entry_file_fixed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DatabaseUse {
    Default,
    OptIn,
    Never,
}

impl AppType {
    pub const ALL: [AppType; 3] = [AppType::Laravel, AppType::Php, AppType::Static];

    pub fn info(self) -> AppTypeInfo {
        match self {
            AppType::Laravel => AppTypeInfo {
                app_type: self,
                id: "laravel",
                title: "Laravel",
                description: "Laravel application with its own MySQL database and shared storage",
                uses_php: true,
                database: DatabaseUse::Default,
                entry_file: "public/index.php",
                entry_file_fixed: true,
            },
            AppType::Php => AppTypeInfo {
                app_type: self,
                id: "php",
                title: "PHP",
                description: "Generic PHP application served through PHP-FPM",
                uses_php: true,
                database: DatabaseUse::OptIn,
                entry_file: "public/index.php",
                entry_file_fixed: false,
            },
            AppType::Static => AppTypeInfo {
                app_type: self,
                id: "static",
                title: "Static HTML",
                description: "Static files served by NGINX",
                uses_php: false,
                database: DatabaseUse::Never,
                entry_file: "index.html",
                entry_file_fixed: false,
            },
        }
    }
}

/// All application types, in display order.
pub fn app_types() -> Vec<AppTypeInfo> {
    AppType::ALL.iter().map(|t| t.info()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_three_types() {
        let ids: Vec<_> = app_types().iter().map(|t| t.id).collect();
        assert_eq!(ids, ["laravel", "php", "static"]);
    }
}
