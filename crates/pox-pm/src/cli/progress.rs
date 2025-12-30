//! Progress reporting for CLI operations.

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::time::Duration;

/// Manages progress bars for downloads and operations
pub struct ProgressManager {
    multi: MultiProgress,
    enabled: bool,
}

impl ProgressManager {
    /// Create a new progress manager
    pub fn new(enabled: bool) -> Self {
        Self {
            multi: MultiProgress::new(),
            enabled,
        }
    }

    /// Create a download progress bar
    pub fn create_download_bar(&self, name: &str, total: u64) -> ProgressBar {
        if !self.enabled {
            return ProgressBar::hidden();
        }

        let pb = self.multi.add(ProgressBar::new(total));
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} {msg}")
                .unwrap()
                .progress_chars("#>-"),
        );
        pb.set_message(name.to_string());
        pb.enable_steady_tick(Duration::from_millis(100));
        pb
    }

    /// Create an operation progress bar (for install/update operations)
    pub fn create_operation_bar(&self, total: u64) -> ProgressBar {
        if !self.enabled {
            return ProgressBar::hidden();
        }

        let pb = self.multi.add(ProgressBar::new(total));
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}")
                .unwrap()
                .progress_chars("#>-"),
        );
        pb.enable_steady_tick(Duration::from_millis(100));
        pb
    }

    /// Create a spinner for indeterminate operations
    pub fn create_spinner(&self, message: &str) -> ProgressBar {
        if !self.enabled {
            return ProgressBar::hidden();
        }

        let pb = self.multi.add(ProgressBar::new_spinner());
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .unwrap(),
        );
        pb.set_message(message.to_string());
        pb.enable_steady_tick(Duration::from_millis(100));
        pb
    }

    /// Create a counting spinner (shows count without total)
    pub fn create_counter(&self, message: &str) -> ProgressBar {
        if !self.enabled {
            return ProgressBar::hidden();
        }

        let pb = self.multi.add(ProgressBar::new_spinner());
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg} ({pos})")
                .unwrap(),
        );
        pb.set_message(message.to_string());
        pb.enable_steady_tick(Duration::from_millis(100));
        pb
    }

    /// Get the multi-progress for advanced usage
    pub fn multi(&self) -> &MultiProgress {
        &self.multi
    }

    /// Check if progress is enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}

impl Default for ProgressManager {
    fn default() -> Self {
        Self::new(true)
    }
}

/// Helper to format bytes for display
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GB");
    }

    #[test]
    fn test_progress_manager_disabled() {
        let pm = ProgressManager::new(false);
        assert!(!pm.is_enabled());
    }
}
