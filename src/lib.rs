//! Portable docking API 1. Results are data proposals; hosts own execution.
pub mod cli_tools;
pub mod connection;
pub mod contracts;
pub mod core;
pub mod http;
pub mod mcp_server;
pub mod package;

pub use contracts::{ContractError, MODEL, describe, parse_request};
pub use core::{Judge, Plugin, candidate_key, judgment_body};

pub mod routing;
pub mod runtime;
pub mod evaluation;
pub mod agent;
