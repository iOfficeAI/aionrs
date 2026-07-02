//! Subcommand implementations for the `aion` CLI binary.
//!
//! This file is a façade — module declarations and re-export only.
//! All dispatch logic lives in `dispatch.rs`.

mod cmd_auth;
mod cmd_config;
mod cmd_session;
mod cmd_skills;
mod dispatch;

pub(crate) use dispatch::dispatch;
