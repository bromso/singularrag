//! singularrag engine: index a repo, rank symbols, render a budgeted map,
//! and record what was served and cut so a human can see it.

#![forbid(unsafe_code)]

pub mod config;
pub mod error;
pub mod fixture;
pub mod index;
pub mod lang;
pub mod secrets;
pub mod store;
pub mod time;
pub mod tokens;
pub mod walk;

pub use error::{Error, Result};
