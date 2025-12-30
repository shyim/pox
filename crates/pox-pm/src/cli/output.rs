//! Output formatting for CLI.

use console::{style, Style, Term};
use std::io::Write;

/// Verbosity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Verbosity {
    Quiet,
    Normal,
    Verbose,
    VeryVerbose,
    Debug,
}

impl Default for Verbosity {
    fn default() -> Self {
        Verbosity::Normal
    }
}

/// Output handler for CLI
pub struct Output {
    term: Term,
    verbosity: Verbosity,
    json_mode: bool,
}

impl Output {
    /// Create a new output handler
    pub fn new() -> Self {
        Self {
            term: Term::stderr(),
            verbosity: Verbosity::Normal,
            json_mode: false,
        }
    }

    /// Set verbosity level
    pub fn set_verbosity(&mut self, verbosity: Verbosity) {
        self.verbosity = verbosity;
    }

    /// Enable JSON output mode
    pub fn set_json_mode(&mut self, json: bool) {
        self.json_mode = json;
    }

    /// Check if output should be shown at given verbosity
    fn should_output(&self, min_verbosity: Verbosity) -> bool {
        !self.json_mode && self.verbosity >= min_verbosity
    }

    /// Write a line
    pub fn writeln(&self, message: &str) {
        if self.should_output(Verbosity::Normal) {
            let _ = writeln!(&self.term, "{}", message);
        }
    }

    /// Write without newline
    pub fn write(&self, message: &str) {
        if self.should_output(Verbosity::Normal) {
            let _ = write!(&self.term, "{}", message);
            let _ = self.term.flush();
        }
    }

    /// Write an info message
    pub fn info(&self, message: &str) {
        if self.should_output(Verbosity::Normal) {
            let _ = writeln!(&self.term, "{}", style(message).cyan());
        }
    }

    /// Write a success message
    pub fn success(&self, message: &str) {
        if self.should_output(Verbosity::Normal) {
            let _ = writeln!(&self.term, "{}", style(message).green());
        }
    }

    /// Write a warning message
    pub fn warning(&self, message: &str) {
        if self.should_output(Verbosity::Quiet) {
            let _ = writeln!(&self.term, "{} {}", style("Warning:").yellow().bold(), message);
        }
    }

    /// Write an error message
    pub fn error(&self, message: &str) {
        let _ = writeln!(&self.term, "{} {}", style("Error:").red().bold(), message);
    }

    /// Write a verbose message
    pub fn verbose(&self, message: &str) {
        if self.should_output(Verbosity::Verbose) {
            let _ = writeln!(&self.term, "{}", style(message).dim());
        }
    }

    /// Write a debug message
    pub fn debug(&self, message: &str) {
        if self.should_output(Verbosity::Debug) {
            let _ = writeln!(&self.term, "{} {}", style("[DEBUG]").magenta(), message);
        }
    }

    /// Write a section header
    pub fn section(&self, title: &str) {
        if self.should_output(Verbosity::Normal) {
            let _ = writeln!(&self.term, "\n{}", style(title).bold().underlined());
        }
    }

    /// Write a list item
    pub fn list_item(&self, prefix: &str, message: &str) {
        if self.should_output(Verbosity::Normal) {
            let _ = writeln!(&self.term, "  {} {}", style(prefix).green(), message);
        }
    }

    /// Write a table row
    pub fn table_row(&self, columns: &[&str], widths: &[usize]) {
        if self.should_output(Verbosity::Normal) {
            let mut line = String::new();
            for (i, col) in columns.iter().enumerate() {
                let width = widths.get(i).copied().unwrap_or(20);
                line.push_str(&format!("{:<width$}", col, width = width));
            }
            let _ = writeln!(&self.term, "{}", line);
        }
    }

    /// Write JSON output
    pub fn json<T: serde::Serialize>(&self, data: &T) {
        if self.json_mode {
            if let Ok(json) = serde_json::to_string_pretty(data) {
                println!("{}", json);
            }
        }
    }

    /// Get the terminal
    pub fn term(&self) -> &Term {
        &self.term
    }

    /// Check if in quiet mode
    pub fn is_quiet(&self) -> bool {
        self.verbosity == Verbosity::Quiet
    }

    /// Check if in JSON mode
    pub fn is_json(&self) -> bool {
        self.json_mode
    }
}

impl Default for Output {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verbosity_ordering() {
        assert!(Verbosity::Quiet < Verbosity::Normal);
        assert!(Verbosity::Normal < Verbosity::Verbose);
        assert!(Verbosity::Verbose < Verbosity::VeryVerbose);
        assert!(Verbosity::VeryVerbose < Verbosity::Debug);
    }

    #[test]
    fn test_output_creation() {
        let output = Output::new();
        assert!(!output.is_quiet());
        assert!(!output.is_json());
    }
}
