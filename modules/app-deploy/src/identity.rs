//! Names derived from an application slug. The `ossi-` and `ossi_`
//! conventions live here, not in Core.

use ossiframe_core::{AppSlug, LinuxUserName, ValidationError};

/// Longest slug used for apps. `ossi-<slug>` must fit the 32 character Linux
/// user limit and `ossi_<slug>` the 32 character MySQL user limit.
pub const MAX_APP_SLUG_LEN: usize = 27;

/// Parses a slug and applies the application length cap.
pub fn app_slug(input: &str) -> Result<AppSlug, ValidationError> {
    let slug = AppSlug::parse(input)?;
    if slug.as_str().len() > MAX_APP_SLUG_LEN {
        return Err(ValidationError::new(
            "slug",
            "must be at most 27 characters so the generated user names fit 32 characters",
        ));
    }
    Ok(slug)
}

/// `ossi-<slug>`.
pub fn app_user(slug: &AppSlug) -> Result<LinuxUserName, ValidationError> {
    if slug.as_str().len() > MAX_APP_SLUG_LEN {
        return Err(ValidationError::new(
            "slug",
            "must be at most 27 characters",
        ));
    }
    LinuxUserName::parse(&format!("ossi-{}", slug.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_cap_and_user_name() {
        let ok = "a".repeat(27);
        assert_eq!(
            app_user(&app_slug(&ok).unwrap()).unwrap().as_str().len(),
            32
        );
        assert!(app_slug(&"a".repeat(28)).is_err());
        // Core still accepts 32; the user builder refuses it.
        assert!(app_user(&AppSlug::parse(&"a".repeat(32)).unwrap()).is_err());
        assert_eq!(
            app_user(&app_slug("blog").unwrap()).unwrap().as_str(),
            "ossi-blog"
        );
    }
}
