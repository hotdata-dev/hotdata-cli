//! Per-command handlers. Each submodule owns the implementation for one
//! top-level CLI command; the clap argument definitions currently live in
//! `crate::cli` (distributing them here is a later step).

pub mod connections;
pub mod context;
pub mod databases;
pub mod embedding_providers;
pub mod indexes;
pub mod jobs;
pub mod queries;
pub mod query;
pub mod results;
pub mod skill;
pub mod tables;
pub mod update;
pub mod usage;
pub mod workspace;
