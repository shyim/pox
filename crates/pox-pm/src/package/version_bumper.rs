use regex::Regex;

pub fn bump_requirement(constraint: &str, installed_version: &str) -> String {
    let constraint = constraint.trim();

    if constraint.starts_with("dev-") {
        return constraint.to_string();
    }

    if installed_version.starts_with("dev-") {
        return constraint.to_string();
    }

    let version = clean_version(installed_version);

    if !is_stable_version(&version) {
        return constraint.to_string();
    }

    let major = get_major_version(&version);
    let new_constraint = bump_constraint_parts(constraint, &version, &major);

    if constraints_equivalent(constraint, &new_constraint) {
        return constraint.to_string();
    }

    new_constraint
}

fn clean_version(version: &str) -> String {
    let version = version.trim();
    let version = version.strip_prefix('v').unwrap_or(version);
    let version = version.strip_prefix('V').unwrap_or(version);
    let version = version.strip_suffix("-dev").unwrap_or(version);
    let version = version
        .trim_end_matches(".0")
        .trim_end_matches(".9999999");

    let version = if let Some(pos) = version.find("-dev") {
        &version[..pos]
    } else {
        version
    };

    let version = if let Some(pos) = version.find("-alpha") {
        &version[..pos]
    } else {
        version
    };
    let version = if let Some(pos) = version.find("-beta") {
        &version[..pos]
    } else {
        version
    };
    let version = if let Some(pos) = version.find("-RC") {
        &version[..pos]
    } else {
        version
    };

    version.to_string()
}

fn is_stable_version(version: &str) -> bool {
    let lower = version.to_lowercase();
    !lower.contains("alpha")
        && !lower.contains("beta")
        && !lower.contains("-rc")
        && !lower.contains("dev")
        && !lower.contains("snapshot")
}

fn get_major_version(version: &str) -> String {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.is_empty() {
        return version.to_string();
    }

    if parts[0] == "0" && parts.len() > 1 {
        format!("0\\.{}", regex::escape(parts[1]))
    } else {
        regex::escape(parts[0])
    }
}

fn strip_trailing_zeros(version: &str) -> String {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() <= 2 {
        return version.to_string();
    }

    let mut keep = 2;
    for (i, part) in parts.iter().enumerate().skip(2) {
        if *part != "0" {
            keep = i + 1;
        }
    }

    parts[..keep].join(".")
}

fn bump_constraint_parts(constraint: &str, version: &str, major: &str) -> String {
    if constraint.contains("||") {
        let parts: Vec<&str> = constraint.split("||").collect();
        let bumped: Vec<String> = parts
            .into_iter()
            .map(|p| bump_single_constraint(p.trim(), version, major))
            .collect();
        return bumped.join(" || ");
    }

    bump_single_constraint(constraint, version, major)
}

fn bump_single_constraint(constraint: &str, version: &str, major: &str) -> String {
    let constraint = constraint.trim();

    if constraint == "*" || constraint.starts_with("*@") {
        let suffix = if constraint.len() > 1 {
            &constraint[1..]
        } else {
            ""
        };
        let replacement = compute_replacement("*", version);
        return format!("{}{}", replacement, suffix);
    }

    let pattern = format!(
        r"(?x)
        (?P<constraint>
            \^v?{major}(?:\.\d+)* # caret constraint like ^2.x.y
            | ~v?{major}(?:\.\d+){{1,3}} # tilde constraint like ~2.2 or ~2.2.2
            | v?{major}(?:\.[*xX])+ # wildcard like 2.* or 2.x.x (only at major level)
            | >=v?\d+(?:\.\d+)* # greater-or-equal like >=2.0
        )
        (?P<suffix>@\w+)? # stability suffix like @dev
        ",
        major = major
    );

    let re = match Regex::new(&pattern) {
        Ok(r) => r,
        Err(_) => return constraint.to_string(),
    };

    if !re.is_match(constraint) {
        return constraint.to_string();
    }

    let mut result = constraint.to_string();
    let mut offset: i64 = 0;

    let matches: Vec<_> = re.captures_iter(constraint).collect();

    for caps in matches.iter().rev() {
        if let Some(m) = caps.name("constraint") {
            let old_constraint = m.as_str();
            let start = m.start() as i64 + offset;
            let end = if let Some(suffix_match) = caps.name("suffix") {
                suffix_match.end() as i64 + offset
            } else {
                m.end() as i64 + offset
            };

            let replacement = compute_replacement(old_constraint, version);
            let suffix = caps.name("suffix").map(|s| s.as_str()).unwrap_or("");

            let new_part = format!("{}{}", replacement, suffix);

            result = format!(
                "{}{}{}",
                &result[..(start as usize)],
                new_part,
                &result[(end as usize)..]
            );

            offset += new_part.len() as i64 - (end - start);
        }
    }

    result
}

fn compute_replacement(old_constraint: &str, version: &str) -> String {
    let old = old_constraint.trim();
    let clean_version = strip_trailing_zeros(version);

    let old_dot_count = old.matches('.').count();
    let clean_version_dot_count = clean_version.matches('.').count();

    let suffix = if old_dot_count == 2 && clean_version_dot_count == 1 {
        ".0"
    } else {
        ""
    };

    if old.starts_with('~') {
        if old_dot_count >= 2 && !old.contains('*') && !old.contains('x') {
            let version_parts: Vec<&str> = clean_version.split('.').collect();
            let mut result_parts: Vec<&str> = version_parts.clone();

            while result_parts.len() <= old_dot_count {
                result_parts.push("0");
            }

            let result: Vec<&str> = result_parts.into_iter().take(old_dot_count + 1).collect();
            return format!("~{}", result.join("."));
        }
        return format!("^{}{}", clean_version, suffix);
    }

    if old.starts_with('^') {
        if old_dot_count >= 2 {
            let version_parts: Vec<&str> = clean_version.split('.').collect();
            let mut result_parts: Vec<&str> = version_parts.clone();

            while result_parts.len() <= old_dot_count {
                result_parts.push("0");
            }

            let result: Vec<&str> = result_parts.into_iter().take(old_dot_count + 1).collect();
            return format!("^{}", result.join("."));
        }

        return format!("^{}{}", clean_version, suffix);
    }

    if old.starts_with(">=") {
        return format!(">={}{}", clean_version, suffix);
    }

    if old == "*" {
        return format!(">={}{}", clean_version, suffix);
    }

    if old.contains('*') || old.contains('x') || old.contains('X') {
        return format!("^{}{}", clean_version, suffix);
    }

    format!("^{}{}", clean_version, suffix)
}

fn constraints_equivalent(old: &str, new: &str) -> bool {
    let old_normalized = old.replace('v', "").replace('V', "");
    let new_normalized = new.replace('v', "").replace('V', "");

    if old_normalized == new_normalized {
        return true;
    }

    let old_stripped = strip_trailing_zeros_in_constraint(&old_normalized);
    let new_stripped = strip_trailing_zeros_in_constraint(&new_normalized);

    old_stripped == new_stripped
}

fn strip_trailing_zeros_in_constraint(constraint: &str) -> String {
    let re = Regex::new(r"(\d+(?:\.\d+)*)").unwrap();
    re.replace_all(constraint, |caps: &regex::Captures| {
        let version = &caps[1];
        let parts: Vec<&str> = version.split('.').collect();
        if parts.len() <= 1 {
            return version.to_string();
        }

        let mut keep = 1;
        for (i, part) in parts.iter().enumerate() {
            if *part != "0" {
                keep = i + 1;
            }
        }

        parts[..keep].join(".")
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test cases from Composer's VersionBumperTest.php
    #[test]
    fn test_upgrade_caret() {
        assert_eq!(bump_requirement("^1.0", "1.2.1"), "^1.2.1");
    }

    #[test]
    fn test_upgrade_caret_with_v() {
        assert_eq!(bump_requirement("^v1.0", "1.2.1"), "^1.2.1");
    }

    #[test]
    fn test_skip_trailing_0s() {
        assert_eq!(bump_requirement("^1.0", "1.0.0"), "^1.0");
        assert_eq!(bump_requirement("^1.2", "1.2.0"), "^1.2");
    }

    #[test]
    fn test_preserve_major_minor_patch_format() {
        assert_eq!(bump_requirement("^1.0.0", "1.2.0"), "^1.2.0");
        assert_eq!(bump_requirement("^1.0.0", "1.2.1"), "^1.2.1");
    }

    #[test]
    fn test_preserve_multi_constraints() {
        assert_eq!(bump_requirement("^1.2 || ^2.3", "1.3.2"), "^1.3.2 || ^2.3");
        assert_eq!(bump_requirement("^1.2 || ^2.3", "2.4.0"), "^1.2 || ^2.4");
        assert_eq!(
            bump_requirement("^1.2 || ^2.3 || ^2", "2.4.0"),
            "^1.2 || ^2.4 || ^2.4"
        );
        assert_eq!(
            bump_requirement("^1.2 || ^2.3.3 || ^2", "2.4.0"),
            "^1.2 || ^2.4.0 || ^2.4"
        );
    }

    #[test]
    fn test_dev_at_suffix_preserved() {
        assert_eq!(bump_requirement("^3@dev", "3.2.0"), "^3.2@dev");
    }

    #[test]
    fn test_non_stable_versions_abort_upgrades() {
        assert_eq!(bump_requirement("~2", "2.1-beta.1"), "~2");
    }

    #[test]
    fn test_dev_reqs_skipped() {
        assert_eq!(bump_requirement("dev-main", "dev-foo"), "dev-main");
    }

    #[test]
    fn test_dev_version_does_not_upgrade() {
        assert_eq!(bump_requirement("^3.2", "dev-main"), "^3.2");
    }

    #[test]
    fn test_upgrade_major_wildcard_to_caret() {
        assert_eq!(bump_requirement("2.*", "2.4.0"), "^2.4");
    }

    #[test]
    fn test_upgrade_major_wildcard_to_caret_with_v() {
        assert_eq!(bump_requirement("v2.*", "2.4.0"), "^2.4");
    }

    #[test]
    fn test_upgrade_major_wildcard_x_to_caret() {
        assert_eq!(bump_requirement("2.x", "2.4.0"), "^2.4");
        assert_eq!(bump_requirement("2.x.x", "2.4.0"), "^2.4.0");
    }

    #[test]
    fn test_leave_minor_wildcard_alone() {
        assert_eq!(bump_requirement("2.4.*", "2.4.3"), "2.4.*");
    }

    #[test]
    fn test_leave_patch_wildcard_alone() {
        assert_eq!(bump_requirement("2.4.3.*", "2.4.3.2"), "2.4.3.*");
    }

    #[test]
    fn test_leave_single_tilde_alone() {
        assert_eq!(bump_requirement("~2", "2.4.3"), "~2");
    }

    #[test]
    fn test_upgrade_tilde_to_caret_when_compatible() {
        assert_eq!(bump_requirement("~2.2", "2.4.3"), "^2.4.3");
    }

    #[test]
    fn test_upgrade_patch_only_tilde() {
        assert_eq!(bump_requirement("~2.2.3", "2.2.6.2"), "~2.2.6");
        assert_eq!(bump_requirement("~2.2.3", "2.2.6"), "~2.2.6");
        assert_eq!(bump_requirement("~2.0.0", "2.0.0"), "~2.0.0");
    }

    #[test]
    fn test_upgrade_patch_only_tilde_year_based() {
        assert_eq!(bump_requirement("~2025.1.561", "2025.1.583"), "~2025.1.583");
    }

    #[test]
    fn test_upgrade_4_bits_tilde() {
        assert_eq!(bump_requirement("~2.2.3.1", "2.2.4"), "~2.2.4.0");
        assert_eq!(bump_requirement("~2.2.3.1", "2.2.4.0"), "~2.2.4.0");
        assert_eq!(bump_requirement("~2.2.3.1", "2.2.4.5"), "~2.2.4.5");
    }

    #[test]
    fn test_upgrade_bigger_or_eq() {
        assert_eq!(bump_requirement(">=3.0", "3.4.5"), ">=3.4.5");
    }

    #[test]
    fn test_upgrade_bigger_or_eq_with_v() {
        assert_eq!(bump_requirement(">=v3.0", "3.4.5"), ">=3.4.5");
    }

    #[test]
    fn test_leave_bigger_than_untouched() {
        assert_eq!(bump_requirement(">2.2.3", "2.2.6"), ">2.2.3");
    }

    #[test]
    fn test_skip_pre_stable_releases() {
        assert_eq!(bump_requirement("^0.3 || ^0.4", "0.4.3"), "^0.3 || ^0.4.3");
    }

    #[test]
    fn test_upgrade_full_wildcard() {
        assert_eq!(bump_requirement("*", "1.2.3"), ">=1.2.3");
    }

    #[test]
    fn test_clean_version() {
        assert_eq!(clean_version("1.2.3"), "1.2.3");
        assert_eq!(clean_version("v1.2.3"), "1.2.3");
        assert_eq!(clean_version("1.2.0"), "1.2");
        assert_eq!(clean_version("1.0.0"), "1");
        assert_eq!(clean_version("1.2.3-dev"), "1.2.3");
        assert_eq!(clean_version("1.2.3.9999999-dev"), "1.2.3");
    }

    #[test]
    fn test_strip_trailing_zeros() {
        assert_eq!(strip_trailing_zeros("1.2.3"), "1.2.3");
        assert_eq!(strip_trailing_zeros("1.2.0"), "1.2");
        assert_eq!(strip_trailing_zeros("1.0.0"), "1.0");
        assert_eq!(strip_trailing_zeros("1.0"), "1.0");
    }
}
