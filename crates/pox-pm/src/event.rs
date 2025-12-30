//! Event system for Composer lifecycle hooks.
//!
//! Each event type has its own struct with appropriate fields.
//! All events implement the `ComposerEvent` trait.

use std::any::Any;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::package::Package;

/// Trait implemented by all Composer events.
pub trait ComposerEvent: Send + Sync + Any {
    /// Returns the event type identifier.
    fn event_type(&self) -> EventType;

    /// Returns the script name for this event (as defined in composer.json).
    fn script_name(&self) -> &'static str {
        self.event_type().script_name()
    }

    /// Whether this is a dev-mode operation.
    fn dev_mode(&self) -> bool {
        true
    }

    /// Downcast to a concrete event type.
    fn as_any(&self) -> &dyn Any;
}

/// Composer lifecycle event types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventType {
    PreInstall,
    PostInstall,
    PreUpdate,
    PostUpdate,
    PreAutoloadDump,
    PostAutoloadDump,
    PostStatus,
    PreArchive,
    PostArchive,
    PostRootPackageInstall,
    PostCreateProject,
    PreOperationsExec,
}

impl EventType {
    /// Returns the script name for this event (as defined in composer.json).
    pub fn script_name(&self) -> &'static str {
        match self {
            EventType::PreInstall => "pre-install-cmd",
            EventType::PostInstall => "post-install-cmd",
            EventType::PreUpdate => "pre-update-cmd",
            EventType::PostUpdate => "post-update-cmd",
            EventType::PreAutoloadDump => "pre-autoload-dump",
            EventType::PostAutoloadDump => "post-autoload-dump",
            EventType::PostStatus => "post-status-cmd",
            EventType::PreArchive => "pre-archive-cmd",
            EventType::PostArchive => "post-archive-cmd",
            EventType::PostRootPackageInstall => "post-root-package-install",
            EventType::PostCreateProject => "post-create-project-cmd",
            EventType::PreOperationsExec => "pre-operations-exec",
        }
    }

    /// Returns all event types.
    pub fn all() -> &'static [EventType] {
        &[
            EventType::PreInstall,
            EventType::PostInstall,
            EventType::PreUpdate,
            EventType::PostUpdate,
            EventType::PreAutoloadDump,
            EventType::PostAutoloadDump,
            EventType::PostStatus,
            EventType::PreArchive,
            EventType::PostArchive,
            EventType::PostRootPackageInstall,
            EventType::PostCreateProject,
            EventType::PreOperationsExec,
        ]
    }
}

/// Event fired before installing packages.
#[derive(Debug, Clone, Default)]
pub struct PreInstallEvent {
    pub dev_mode: bool,
}

impl PreInstallEvent {
    pub fn new(dev_mode: bool) -> Self {
        Self { dev_mode }
    }
}

impl ComposerEvent for PreInstallEvent {
    fn event_type(&self) -> EventType {
        EventType::PreInstall
    }

    fn dev_mode(&self) -> bool {
        self.dev_mode
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Event fired after installing packages.
#[derive(Debug, Clone, Default)]
pub struct PostInstallEvent {
    pub dev_mode: bool,
}

impl PostInstallEvent {
    pub fn new(dev_mode: bool) -> Self {
        Self { dev_mode }
    }
}

impl ComposerEvent for PostInstallEvent {
    fn event_type(&self) -> EventType {
        EventType::PostInstall
    }

    fn dev_mode(&self) -> bool {
        self.dev_mode
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Event fired before updating packages.
#[derive(Debug, Clone, Default)]
pub struct PreUpdateEvent {
    pub dev_mode: bool,
}

impl PreUpdateEvent {
    pub fn new(dev_mode: bool) -> Self {
        Self { dev_mode }
    }
}

impl ComposerEvent for PreUpdateEvent {
    fn event_type(&self) -> EventType {
        EventType::PreUpdate
    }

    fn dev_mode(&self) -> bool {
        self.dev_mode
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Event fired after updating packages.
#[derive(Debug, Clone, Default)]
pub struct PostUpdateEvent {
    pub dev_mode: bool,
}

impl PostUpdateEvent {
    pub fn new(dev_mode: bool) -> Self {
        Self { dev_mode }
    }
}

impl ComposerEvent for PostUpdateEvent {
    fn event_type(&self) -> EventType {
        EventType::PostUpdate
    }

    fn dev_mode(&self) -> bool {
        self.dev_mode
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Event fired before dumping autoloader.
#[derive(Debug, Clone, Default)]
pub struct PreAutoloadDumpEvent {
    pub dev_mode: bool,
    pub optimize: bool,
}

impl PreAutoloadDumpEvent {
    pub fn new(dev_mode: bool, optimize: bool) -> Self {
        Self { dev_mode, optimize }
    }
}

impl ComposerEvent for PreAutoloadDumpEvent {
    fn event_type(&self) -> EventType {
        EventType::PreAutoloadDump
    }

    fn dev_mode(&self) -> bool {
        self.dev_mode
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Event fired after dumping autoloader.
#[derive(Debug, Clone)]
pub struct PostAutoloadDumpEvent {
    /// Packages included in the autoload dump.
    pub packages: Vec<Arc<Package>>,
    /// Whether this is a dev-mode operation.
    pub dev_mode: bool,
    /// Whether the autoloader was optimized.
    pub optimize: bool,
}

impl PostAutoloadDumpEvent {
    pub fn new(packages: Vec<Arc<Package>>, dev_mode: bool, optimize: bool) -> Self {
        Self { packages, dev_mode, optimize }
    }
}

impl ComposerEvent for PostAutoloadDumpEvent {
    fn event_type(&self) -> EventType {
        EventType::PostAutoloadDump
    }

    fn dev_mode(&self) -> bool {
        self.dev_mode
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Event fired before executing package operations.
#[derive(Debug, Clone, Default)]
pub struct PreOperationsExecEvent {
    pub dev_mode: bool,
}

impl PreOperationsExecEvent {
    pub fn new(dev_mode: bool) -> Self {
        Self { dev_mode }
    }
}

impl ComposerEvent for PreOperationsExecEvent {
    fn event_type(&self) -> EventType {
        EventType::PreOperationsExec
    }

    fn dev_mode(&self) -> bool {
        self.dev_mode
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Event fired after status command.
#[derive(Debug, Clone, Default)]
pub struct PostStatusEvent {
    pub dev_mode: bool,
}

impl PostStatusEvent {
    pub fn new(dev_mode: bool) -> Self {
        Self { dev_mode }
    }
}

impl ComposerEvent for PostStatusEvent {
    fn event_type(&self) -> EventType {
        EventType::PostStatus
    }

    fn dev_mode(&self) -> bool {
        self.dev_mode
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Event fired before creating an archive.
#[derive(Debug, Clone)]
pub struct PreArchiveEvent {
    pub format: String,
    pub dev_mode: bool,
}

impl PreArchiveEvent {
    pub fn new(format: impl Into<String>, dev_mode: bool) -> Self {
        Self { format: format.into(), dev_mode }
    }
}

impl ComposerEvent for PreArchiveEvent {
    fn event_type(&self) -> EventType {
        EventType::PreArchive
    }

    fn dev_mode(&self) -> bool {
        self.dev_mode
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Event fired after creating an archive.
#[derive(Debug, Clone)]
pub struct PostArchiveEvent {
    pub format: String,
    pub archive_path: PathBuf,
    pub dev_mode: bool,
}

impl PostArchiveEvent {
    pub fn new(format: impl Into<String>, archive_path: PathBuf, dev_mode: bool) -> Self {
        Self { format: format.into(), archive_path, dev_mode }
    }
}

impl ComposerEvent for PostArchiveEvent {
    fn event_type(&self) -> EventType {
        EventType::PostArchive
    }

    fn dev_mode(&self) -> bool {
        self.dev_mode
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Event fired after the root package is installed.
#[derive(Debug, Clone, Default)]
pub struct PostRootPackageInstallEvent {
    pub dev_mode: bool,
}

impl PostRootPackageInstallEvent {
    pub fn new(dev_mode: bool) -> Self {
        Self { dev_mode }
    }
}

impl ComposerEvent for PostRootPackageInstallEvent {
    fn event_type(&self) -> EventType {
        EventType::PostRootPackageInstall
    }

    fn dev_mode(&self) -> bool {
        self.dev_mode
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Event fired after create-project command.
#[derive(Debug, Clone, Default)]
pub struct PostCreateProjectEvent {
    pub dev_mode: bool,
}

impl PostCreateProjectEvent {
    pub fn new(dev_mode: bool) -> Self {
        Self { dev_mode }
    }
}

impl ComposerEvent for PostCreateProjectEvent {
    fn event_type(&self) -> EventType {
        EventType::PostCreateProject
    }

    fn dev_mode(&self) -> bool {
        self.dev_mode
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Trait for event listeners.
///
/// Listeners receive the event, a reference to the Composer instance,
/// and the working directory.
pub trait EventListener: Send + Sync {
    /// Handle an event. Returns the exit code (0 for success).
    fn handle(
        &self,
        event: &dyn ComposerEvent,
        composer: &crate::composer::Composer,
    ) -> anyhow::Result<i32>;

    /// Returns the priority of this listener (higher = earlier execution).
    fn priority(&self) -> i32 {
        0
    }
}

/// Script-based event listener that executes composer.json scripts.
#[derive(Default)]
pub struct ScriptEventListener {
    quiet: bool,
}

impl ScriptEventListener {
    pub fn new() -> Self {
        Self { quiet: false }
    }

    pub fn quiet(mut self, quiet: bool) -> Self {
        self.quiet = quiet;
        self
    }
}

impl EventListener for ScriptEventListener {
    fn handle(
        &self,
        event: &dyn ComposerEvent,
        composer: &crate::composer::Composer,
    ) -> anyhow::Result<i32> {
        crate::scripts::run_event_script(
            event.script_name(),
            &composer.composer_json,
            &composer.working_dir,
            self.quiet,
        )
    }
}

/// Event dispatcher that manages listeners and dispatches events.
pub struct EventDispatcher {
    listeners: HashMap<EventType, Vec<Arc<dyn EventListener>>>,
}

impl EventDispatcher {
    /// Create a new event dispatcher.
    pub fn new() -> Self {
        Self { listeners: HashMap::new() }
    }

    /// Create an event dispatcher with script listeners.
    pub fn with_scripts() -> Self {
        let mut dispatcher = Self::new();
        let listener = Arc::new(ScriptEventListener::new());

        for event_type in EventType::all() {
            dispatcher.add_listener(*event_type, listener.clone());
        }

        dispatcher
    }

    /// Add a listener for a specific event type.
    pub fn add_listener(&mut self, event_type: EventType, listener: Arc<dyn EventListener>) {
        self.listeners.entry(event_type).or_default().push(listener);
    }

    /// Dispatch a typed event to all registered listeners.
    pub fn dispatch<E: ComposerEvent>(
        &self,
        event: &E,
        composer: &crate::composer::Composer,
    ) -> anyhow::Result<i32> {
        let Some(listeners) = self.listeners.get(&event.event_type()) else {
            return Ok(0);
        };

        if listeners.is_empty() {
            return Ok(0);
        }

        let mut sorted_listeners: Vec<_> = listeners.iter().collect();
        sorted_listeners.sort_by(|a, b| b.priority().cmp(&a.priority()));

        for listener in sorted_listeners {
            let exit_code = listener.handle(event, composer)?;
            if exit_code != 0 {
                return Ok(exit_code);
            }
        }

        Ok(0)
    }
}

impl Default for EventDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_type_script_names() {
        assert_eq!(EventType::PreInstall.script_name(), "pre-install-cmd");
        assert_eq!(EventType::PostInstall.script_name(), "post-install-cmd");
        assert_eq!(EventType::PreUpdate.script_name(), "pre-update-cmd");
        assert_eq!(EventType::PostUpdate.script_name(), "post-update-cmd");
    }

    #[test]
    fn test_typed_event_properties() {
        let event = PostAutoloadDumpEvent::new(
            vec![Arc::new(Package::new("vendor/package", "1.0.0"))],
            true,
            false,
        );

        assert_eq!(event.event_type(), EventType::PostAutoloadDump);
        assert_eq!(event.script_name(), "post-autoload-dump");
        assert_eq!(event.packages.len(), 1);
        assert!(event.dev_mode());
    }

    #[test]
    fn test_archive_events() {
        let pre = PreArchiveEvent::new("zip", true);
        assert_eq!(pre.format, "zip");
        assert_eq!(pre.event_type(), EventType::PreArchive);

        let post = PostArchiveEvent::new("tar", PathBuf::from("/path/to/archive.tar"), true);
        assert_eq!(post.format, "tar");
        assert_eq!(post.archive_path, PathBuf::from("/path/to/archive.tar"));
        assert_eq!(post.event_type(), EventType::PostArchive);
    }

    #[test]
    fn test_event_dispatcher_new() {
        let dispatcher = EventDispatcher::new();
        assert!(dispatcher.listeners.is_empty());
    }

    #[test]
    fn test_event_dispatcher_add_listener() {
        struct DummyListener;
        impl EventListener for DummyListener {
            fn handle(&self, _: &dyn ComposerEvent, _: &crate::composer::Composer) -> anyhow::Result<i32> {
                Ok(0)
            }
        }

        let mut dispatcher = EventDispatcher::new();
        dispatcher.add_listener(EventType::PreInstall, Arc::new(DummyListener));
        assert_eq!(dispatcher.listeners.get(&EventType::PreInstall).unwrap().len(), 1);
    }
}
