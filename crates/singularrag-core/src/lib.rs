//! singularrag engine: index a repo, rank symbols, render a budgeted map,
//! and record what was served and cut so a human can see it.

#![forbid(unsafe_code)]

pub mod error;
pub mod time;

pub use error::{Error, Result};
