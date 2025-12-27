//! Plugin registry - provides access to all ported Composer plugins.
//!
//! Each plugin implements `EventListener` directly and checks if its
//! corresponding package is installed before taking action.

use std::sync::Arc;

use crate::event::{EventListener, EventType, EventDispatcher};

use super::composer_bin::ComposerBinPlugin;
use super::phpstan_extension_installer::PhpstanExtensionInstallerPlugin;
use super::symfony_runtime::SymfonyRuntimePlugin;

/// Register all plugins with the event dispatcher.
///
/// Plugins check themselves whether their package is installed
/// before taking any action.
pub fn register_plugins(dispatcher: &mut EventDispatcher) {
    dispatcher.add_listener(EventType::PostAutoloadDump, Arc::new(ComposerBinPlugin) as Arc<dyn EventListener>);
    dispatcher.add_listener(EventType::PostAutoloadDump, Arc::new(PhpstanExtensionInstallerPlugin) as Arc<dyn EventListener>);
    dispatcher.add_listener(EventType::PostAutoloadDump, Arc::new(SymfonyRuntimePlugin) as Arc<dyn EventListener>);
}
