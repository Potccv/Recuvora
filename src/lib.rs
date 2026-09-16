//! Recuvora's trusted core, organized as modules in a single Cargo package.
//!
//! Framework composition, simulation, durable approval, scoped text actions,
//! Harness selection and assembly retain separate responsibilities. External
//! nodes and contract plugins communicate through a bounded protocol. Optional
//! Web and Tauri Desktop share the authenticated console client. Text readback
//! remains distinct from business-health verification.

pub mod actions;
pub mod boot;
pub mod framework;
pub mod harnesses;
pub mod interfaces;
pub mod monitors;
pub mod nodes;
pub mod protocol;
pub mod recovery;
#[cfg(feature = "server")]
pub mod server;
