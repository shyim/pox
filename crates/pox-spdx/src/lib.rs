use lazy_static::lazy_static;
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;

const LICENSES_JSON: &str = include_str!("../res/spdx-licenses.json");
const EXCEPTIONS_JSON: &str = include_str!("../res/spdx-exceptions.json");

/// License information: (identifier, full_name, osi_approved, deprecated)
pub type LicenseInfo = (String, String, bool, bool);

/// Exception information: (identifier, full_name)
pub type ExceptionInfo = (String, String);

/// License result from getLicenseByIdentifier: (full_name, osi_approved, url, deprecated)
pub type LicenseResult = (String, bool, String, bool);

/// Exception result from getExceptionByIdentifier: (full_name, url)
pub type ExceptionResult = (String, String);

pub struct SpdxLicenses {
    licenses: HashMap<String, LicenseInfo>,
    exceptions: HashMap<String, ExceptionInfo>,
}

impl Default for SpdxLicenses {
    fn default() -> Self {
        Self::new()
    }
}

impl SpdxLicenses {
    pub fn new() -> Self {
        let mut instance = Self {
            licenses: HashMap::new(),
            exceptions: HashMap::new(),
        };
        instance.load_licenses();
        instance.load_exceptions();
        instance
    }

    /// Returns license metadata by license identifier.
    ///
    /// Returns: (full_name, osi_approved, url, deprecated)
    pub fn get_license_by_identifier(&self, identifier: &str) -> Option<LicenseResult> {
        let key = identifier.to_lowercase();
        self.licenses.get(&key).map(|license| {
            (
                license.1.clone(),
                license.2,
                format!(
                    "https://spdx.org/licenses/{}.html#licenseText",
                    license.0
                ),
                license.3,
            )
        })
    }

    /// Returns all licenses information, keyed by the lowercased license identifier.
    pub fn get_licenses(&self) -> &HashMap<String, LicenseInfo> {
        &self.licenses
    }

    /// Returns license exception metadata by license exception identifier.
    ///
    /// Returns: (full_name, url)
    pub fn get_exception_by_identifier(&self, identifier: &str) -> Option<ExceptionResult> {
        let key = identifier.to_lowercase();
        self.exceptions.get(&key).map(|exception| {
            (
                exception.1.clone(),
                format!(
                    "https://spdx.org/licenses/{}.html#licenseExceptionText",
                    exception.0
                ),
            )
        })
    }

    /// Returns the short identifier of a license (or license exception) by full name.
    pub fn get_identifier_by_name(&self, name: &str) -> Option<String> {
        for license_data in self.licenses.values() {
            if license_data.1 == name {
                return Some(license_data.0.clone());
            }
        }

        for exception_data in self.exceptions.values() {
            if exception_data.1 == name {
                return Some(exception_data.0.clone());
            }
        }

        None
    }

    /// Returns the OSI Approved status for a license by identifier.
    pub fn is_osi_approved_by_identifier(&self, identifier: &str) -> bool {
        let key = identifier.to_lowercase();
        self.licenses.get(&key).map_or(false, |l| l.2)
    }

    /// Returns the deprecation status for a license by identifier.
    pub fn is_deprecated_by_identifier(&self, identifier: &str) -> bool {
        let key = identifier.to_lowercase();
        self.licenses.get(&key).map_or(false, |l| l.3)
    }

    /// Validates a license string or array of license strings.
    pub fn validate(&self, license: &str) -> bool {
        self.is_valid_license_string(license)
    }

    /// Validates an array of license strings (combined with OR).
    pub fn validate_array(&self, licenses: &[&str]) -> bool {
        if licenses.is_empty() {
            return false;
        }
        let license = if licenses.len() > 1 {
            format!("({})", licenses.join(" OR "))
        } else {
            licenses[0].to_string()
        };
        self.is_valid_license_string(&license)
    }

    fn load_licenses(&mut self) {
        let json: HashMap<String, Value> =
            serde_json::from_str(LICENSES_JSON).expect("Failed to parse licenses JSON");

        for (identifier, license) in json {
            let arr = license.as_array().expect("License should be an array");
            let name = arr[0].as_str().expect("Name should be a string").to_string();
            let osi_approved = arr[1].as_bool().expect("OSI approved should be a bool");
            let deprecated = arr[2].as_bool().expect("Deprecated should be a bool");

            self.licenses.insert(
                identifier.to_lowercase(),
                (identifier, name, osi_approved, deprecated),
            );
        }
    }

    fn load_exceptions(&mut self) {
        let json: HashMap<String, Value> =
            serde_json::from_str(EXCEPTIONS_JSON).expect("Failed to parse exceptions JSON");

        for (identifier, exception) in json {
            let arr = exception.as_array().expect("Exception should be an array");
            let name = arr[0].as_str().expect("Name should be a string").to_string();

            self.exceptions
                .insert(identifier.to_lowercase(), (identifier, name));
        }
    }

    fn is_valid_license_string(&self, license: &str) -> bool {
        // Check if it's a direct license identifier
        if self.licenses.contains_key(&license.to_lowercase()) {
            return true;
        }

        // The Rust regex crate doesn't support recursive patterns,
        // so we use a simple parser instead
        self.parse_license_expression(license)
    }

    fn parse_license_expression(&self, expr: &str) -> bool {
        let expr = expr.trim();

        if expr.is_empty() {
            return false;
        }

        // Handle NONE and NOASSERTION
        if expr.eq_ignore_ascii_case("NONE") || expr.eq_ignore_ascii_case("NOASSERTION") {
            return true;
        }

        // Try to parse as a compound expression
        self.parse_compound_expression(expr)
    }

    fn parse_compound_expression(&self, expr: &str) -> bool {
        let expr = expr.trim();

        if expr.is_empty() {
            return false;
        }

        // Try to split by OR (lowest precedence)
        if let Some((left, right)) = self.split_by_operator(expr, "OR") {
            return self.parse_compound_expression(left) && self.parse_compound_expression(right);
        }

        // Try to split by AND
        if let Some((left, right)) = self.split_by_operator(expr, "AND") {
            return self.parse_compound_expression(left) && self.parse_compound_expression(right);
        }

        // Check for parenthesized expression
        if expr.starts_with('(') && expr.ends_with(')') {
            return self.parse_compound_expression(&expr[1..expr.len() - 1]);
        }

        // Check for simple expression with WITH clause
        if let Some((license, exception)) = self.split_by_with(expr) {
            return self.is_valid_simple_expression(license) && self.is_valid_exception(exception);
        }

        // Simple expression
        self.is_valid_simple_expression(expr)
    }

    fn split_by_operator<'a>(&self, expr: &'a str, op: &str) -> Option<(&'a str, &'a str)> {
        let mut depth = 0;
        let op_pattern = format!(" {} ", op);

        let bytes = expr.as_bytes();
        let mut i = 0;

        while i < expr.len() {
            let c = bytes[i] as char;

            if c == '(' {
                depth += 1;
            } else if c == ')' {
                depth -= 1;
            } else if depth == 0 && i + op_pattern.len() <= expr.len() {
                let slice = &expr[i..i + op_pattern.len()];
                if slice.eq_ignore_ascii_case(&op_pattern) {
                    let left = expr[..i].trim();
                    let right = expr[i + op_pattern.len()..].trim();
                    if !left.is_empty() && !right.is_empty() {
                        return Some((left, right));
                    }
                }
            }

            i += 1;
        }

        None
    }

    fn split_by_with<'a>(&self, expr: &'a str) -> Option<(&'a str, &'a str)> {
        // Find " WITH " (case insensitive)
        let lower = expr.to_lowercase();
        if let Some(pos) = lower.find(" with ") {
            let license = expr[..pos].trim();
            let exception = expr[pos + 6..].trim();
            if !license.is_empty() && !exception.is_empty() {
                return Some((license, exception));
            }
        }
        None
    }

    fn is_valid_simple_expression(&self, expr: &str) -> bool {
        let expr = expr.trim();

        // Check for license identifier with optional +
        let check_expr = if expr.ends_with('+') {
            &expr[..expr.len() - 1]
        } else {
            expr
        };

        // Check if it's a valid license identifier
        if self.licenses.contains_key(&check_expr.to_lowercase()) {
            return true;
        }

        // Check for LicenseRef pattern
        lazy_static! {
            static ref LICENSE_REF_RE: Regex =
                Regex::new(r"^(?i)(?:DocumentRef-[\w.\-]+:)?LicenseRef-[\w.\-]+$").unwrap();
        }

        LICENSE_REF_RE.is_match(expr)
    }

    fn is_valid_exception(&self, exception: &str) -> bool {
        self.exceptions.contains_key(&exception.to_lowercase())
    }
}

#[cfg(test)]
mod tests;
