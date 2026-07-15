//! JSON stream protocol entry point for host integration (e.g. AionUI).
//!
//! This file is a façade — module declarations and re-export only.
//! All logic lives in the submodules:
//! - `session`: setup + the two-phase orchestration loop
//! - `pre_message`: the `AddMcpServer`-only phase before the first `Message`
//! - `dispatch`: top-level `ProtocolCommand` handling (outside a `Message`)
//! - `message`: `Message` handling, including the inner command-select loop

mod context;
mod dispatch;
mod message;
mod pre_message;
mod session;

pub(crate) use session::run;
