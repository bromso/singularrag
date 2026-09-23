//! singularrag engine: index a repo, rank symbols, render a budgeted map,
//! and record what was served and cut so a human can see it.

#![deny(unsafe_code)]

pub mod blast;
pub mod changed;
pub mod config;
pub mod doc;
pub mod engine;
pub mod error;
pub mod eval;
pub mod fake_ollama;
pub mod find;
pub mod fixture;
pub mod graph;
pub mod index;
pub mod lang;
pub mod map;
pub mod models;
pub mod rank;
pub mod secrets;
pub mod store;
pub mod time;
pub mod tokens;
pub mod trace;
pub mod walk;
pub mod workspace;

pub use error::{Error, Result};
