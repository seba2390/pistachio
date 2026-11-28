//! A local model for this computer: what it can run well, and getting it running.
//!
//! [`hardware::scan`] looks at the computer (memory, chip, graphics, free disk). [`recommend`]
//! picks four suggestions from the [`catalog`] (tiny, fast, balanced, most capable), each a model plus
//! the engine that runs it best here: a default that installs unattended, and, where one is
//! faster, a specialized engine that takes more steps (a local build). [`install`] downloads what
//! a suggestion needs, every file pinned and checked against its SHA-256, and [`Server`] runs it
//! for this computer only.
//!
//! The catalog is a copy of `catalog/catalog.json` from local-llm-radar
//! (`script/update-local-models.sh` moves it), reviewed like any other change: it decides what
//! is downloaded and run.
//!
//! Everything is below one folder, `root`:
//! - `engines/<id>-<version>/`: an engine, moved into place when it is complete;
//! - `models/<model>/<build>-<revision>/`: a model's files, checked, then moved into place;
//! - `setups.json`: what was set up (model, build, engine, port), so it can be started again.

pub mod catalog;
pub mod hardware;
pub mod install;
pub mod recommend;
mod server;
pub mod setups;
pub mod thinking;

pub use catalog::{Catalog, catalog};
pub use hardware::Hardware;
pub use install::{InstallError, Progress, Stage};
pub use recommend::{Choice, Suggestion, Tier, suggest};
pub use server::Server;
pub use setups::Setup;
