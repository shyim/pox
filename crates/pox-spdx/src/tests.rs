use super::*;

fn licenses() -> SpdxLicenses {
    SpdxLicenses::new()
}

// Test valid licenses
#[test]
fn test_validate_mit() {
    assert!(licenses().validate("MIT"));
}

#[test]
fn test_validate_mit_plus() {
    assert!(licenses().validate("MIT+"));
}

#[test]
fn test_validate_mit_parenthesized() {
    assert!(licenses().validate("(MIT)"));
}

#[test]
fn test_validate_none() {
    assert!(licenses().validate("NONE"));
}

#[test]
fn test_validate_noassertion() {
    assert!(licenses().validate("NOASSERTION"));
}

#[test]
fn test_validate_license_ref() {
    assert!(licenses().validate("LicenseRef-3"));
}

#[test]
fn test_validate_array_lgpl_gpl() {
    assert!(licenses().validate_array(&["LGPL-2.0-only", "GPL-3.0-or-later"]));
}

#[test]
fn test_validate_lgpl_or_gpl_lowercase() {
    assert!(licenses().validate("(LGPL-2.0-only or GPL-3.0-or-later)"));
}

#[test]
fn test_validate_lgpl_or_gpl_uppercase() {
    assert!(licenses().validate("(LGPL-2.0-only OR GPL-3.0-or-later)"));
}

#[test]
fn test_validate_array_eudatagrid_and_gpl() {
    assert!(licenses().validate_array(&["EUDatagrid and GPL-3.0-or-later"]));
}

#[test]
fn test_validate_eudatagrid_and_gpl_lowercase() {
    assert!(licenses().validate("(EUDatagrid and GPL-3.0-or-later)"));
}

#[test]
fn test_validate_eudatagrid_and_gpl_uppercase() {
    assert!(licenses().validate("(EUDatagrid AND GPL-3.0-or-later)"));
}

#[test]
fn test_validate_gpl_with_exception_lowercase() {
    assert!(licenses().validate("GPL-2.0-only with Autoconf-exception-2.0"));
}

#[test]
fn test_validate_gpl_with_exception_uppercase() {
    assert!(licenses().validate("GPL-2.0-only WITH Autoconf-exception-2.0"));
}

#[test]
fn test_validate_gpl_or_later_with_exception() {
    assert!(licenses().validate("GPL-2.0-or-later WITH Autoconf-exception-2.0"));
}

#[test]
fn test_validate_complex_expression() {
    assert!(licenses().validate_array(&["(GPL-3.0-only and GPL-2.0-only or GPL-3.0-or-later)"]));
}

// Test invalid licenses
#[test]
fn test_invalid_empty_string() {
    assert!(!licenses().validate(""));
}

#[test]
fn test_invalid_random_string() {
    assert!(!licenses().validate("The system pwns you"));
}

#[test]
fn test_invalid_empty_parens() {
    assert!(!licenses().validate("()"));
}

#[test]
fn test_invalid_unclosed_paren() {
    assert!(!licenses().validate("(MIT"));
}

#[test]
fn test_invalid_extra_close_paren() {
    assert!(!licenses().validate("MIT)"));
}

#[test]
fn test_invalid_mit_none() {
    assert!(!licenses().validate("MIT NONE"));
}

#[test]
fn test_invalid_mit_and_none() {
    assert!(!licenses().validate("MIT AND NONE"));
}

#[test]
fn test_invalid_mit_mit_and_mit() {
    assert!(!licenses().validate("MIT (MIT and MIT)"));
}

#[test]
fn test_invalid_mit_and_mit_mit() {
    assert!(!licenses().validate("(MIT and MIT) MIT"));
}

#[test]
fn test_invalid_array_with_invalid() {
    assert!(!licenses().validate_array(&["LGPL-2.0-only", "The system pwns you"]));
}

#[test]
fn test_invalid_and_gpl() {
    assert!(!licenses().validate("and GPL-3.0-or-later"));
}

#[test]
fn test_invalid_trailing_and() {
    assert!(!licenses().validate("(EUDatagrid and GPL-3.0-or-later and  )"));
}

#[test]
fn test_invalid_xor_operator() {
    assert!(!licenses().validate("(EUDatagrid xor GPL-3.0-or-later)"));
}

#[test]
fn test_invalid_none_or_mit() {
    assert!(!licenses().validate("(NONE or MIT)"));
}

#[test]
fn test_invalid_noassertion_or_mit() {
    assert!(!licenses().validate("(NOASSERTION or MIT)"));
}

#[test]
fn test_invalid_exception_with_mit() {
    assert!(!licenses().validate("Autoconf-exception-2.0 WITH MIT"));
}

#[test]
fn test_invalid_mit_with_nothing() {
    assert!(!licenses().validate("MIT WITH"));
}

#[test]
fn test_invalid_mit_or_nothing() {
    assert!(!licenses().validate("MIT OR"));
}

#[test]
fn test_invalid_mit_and_nothing() {
    assert!(!licenses().validate("MIT AND"));
}

#[test]
fn test_invalid_empty_array() {
    assert!(!licenses().validate_array(&[]));
}

// Test getLicenseByIdentifier
#[test]
fn test_get_license_by_identifier() {
    let spdx = licenses();
    let license = spdx.get_license_by_identifier("AGPL-1.0-only");
    assert!(license.is_some());
    let license = license.unwrap();
    assert_eq!(license.0, "Affero General Public License v1.0 only");
    assert!(!license.1); // not OSI approved
    assert!(license.2.starts_with("https://spdx.org/licenses/"));
    assert!(!license.3); // not deprecated
}

#[test]
fn test_get_license_by_identifier_invalid() {
    let spdx = licenses();
    let license = spdx.get_license_by_identifier("AGPL-1.0-Illegal");
    assert!(license.is_none());
}

// Test getLicenses
#[test]
fn test_get_licenses() {
    let spdx = licenses();
    let results = spdx.get_licenses();

    assert!(results.contains_key("cc-by-sa-4.0"));
    let cc_by_sa = results.get("cc-by-sa-4.0").unwrap();
    assert_eq!(cc_by_sa.0, "CC-BY-SA-4.0");
    assert_eq!(cc_by_sa.1, "Creative Commons Attribution Share Alike 4.0 International");
    assert!(!cc_by_sa.2); // not OSI approved
    assert!(!cc_by_sa.3); // not deprecated
}

// Test getExceptionByIdentifier
#[test]
fn test_get_exception_by_identifier_invalid() {
    let spdx = licenses();
    let exception = spdx.get_exception_by_identifier("Font-exception-2.0-Errorl");
    assert!(exception.is_none());
}

#[test]
fn test_get_exception_by_identifier() {
    let spdx = licenses();
    let exception = spdx.get_exception_by_identifier("Font-exception-2.0");
    assert!(exception.is_some());
    let exception = exception.unwrap();
    assert_eq!(exception.0, "Font exception 2.0");
}

// Test getIdentifierByName
#[test]
fn test_get_identifier_by_name_agpl() {
    let spdx = licenses();
    let identifier = spdx.get_identifier_by_name("Affero General Public License v1.0");
    assert_eq!(identifier, Some("AGPL-1.0".to_string()));
}

#[test]
fn test_get_identifier_by_name_bsd() {
    let spdx = licenses();
    let identifier = spdx.get_identifier_by_name("BSD 2-Clause \"Simplified\" License");
    assert_eq!(identifier, Some("BSD-2-Clause".to_string()));
}

#[test]
fn test_get_identifier_by_name_exception() {
    let spdx = licenses();
    let identifier = spdx.get_identifier_by_name("Font exception 2.0");
    assert_eq!(identifier, Some("Font-exception-2.0".to_string()));
}

#[test]
fn test_get_identifier_by_name_not_found() {
    let spdx = licenses();
    let identifier = spdx.get_identifier_by_name("null-identifier-name");
    assert!(identifier.is_none());
}

// Test isOsiApprovedByIdentifier
#[test]
fn test_is_osi_approved_mit() {
    let spdx = licenses();
    assert!(spdx.is_osi_approved_by_identifier("MIT"));
}

#[test]
fn test_is_osi_approved_agpl() {
    let spdx = licenses();
    assert!(!spdx.is_osi_approved_by_identifier("AGPL-1.0"));
}

// Test isDeprecatedByIdentifier
#[test]
fn test_is_deprecated_gpl3() {
    let spdx = licenses();
    assert!(spdx.is_deprecated_by_identifier("GPL-3.0"));
}

#[test]
fn test_is_deprecated_gpl3_only() {
    let spdx = licenses();
    assert!(!spdx.is_deprecated_by_identifier("GPL-3.0-only"));
}

// Test that all license identifiers from the JSON file are valid
#[test]
fn test_all_license_identifiers_are_valid() {
    let spdx = licenses();
    for (_, license_info) in spdx.get_licenses() {
        assert!(
            spdx.validate(&license_info.0),
            "License identifier '{}' should be valid",
            license_info.0
        );
    }
}

// Test case insensitivity
#[test]
fn test_case_insensitivity_mit() {
    let spdx = licenses();
    assert!(spdx.validate("mit"));
    assert!(spdx.validate("MIT"));
    assert!(spdx.validate("Mit"));
}

#[test]
fn test_case_insensitivity_operators() {
    let spdx = licenses();
    assert!(spdx.validate("(MIT or GPL-3.0-only)"));
    assert!(spdx.validate("(MIT OR GPL-3.0-only)"));
    assert!(spdx.validate("(MIT and GPL-3.0-only)"));
    assert!(spdx.validate("(MIT AND GPL-3.0-only)"));
}
