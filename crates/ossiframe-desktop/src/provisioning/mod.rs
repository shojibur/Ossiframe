//! Provisioning engine pieces that live in the desktop crate.
//!
//! Compiled only with the `provisioning` feature. An app opts in with an
//! explicit `provisioning::setup(app, provisioner)` call after
//! `tauri_support::setup`, and lists `commands::PROVISION_COMMAND_NAMES`
//! next to the shared command names.
pub mod commands;
pub mod engine;
#[cfg(test)]
mod engine_tests;
pub mod plans;
pub mod reconcile;

pub use commands::{PROVISION_COMMAND_NAMES, Provisioning, setup};
