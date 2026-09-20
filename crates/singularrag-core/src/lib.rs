//! singularrag engine: index a repo, rank symbols, render a budgeted map,
//! and record what was served and cut so a human can see it.

#![forbid(unsafe_code)]

pub mod blast;
pub mod config;
pub mod engine;
pub mod error;
pub mod eval;
pub mod find;
pub mod fixture;
pub mod graph;
pub mod index;
pub mod lang;
pub mod map;
pub mod rank;
pub mod secrets;
pub mod store;
pub mod time;
pub mod tokens;
pub mod walk;

pub use error::{Error, Result};
