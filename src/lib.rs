//! Portable docking API 1. Results are data proposals; hosts own execution.
pub mod cli_tools;
pub mod connect;
pub mod connection;
pub mod contracts;
pub mod core;
pub mod http;
pub mod mcp_server;
pub mod package;

pub use contracts::{ContractError, MODEL, describe, parse_request};
pub use core::{Judge, Plugin, candidate_key, judgment_body};

pub mod agent;
pub mod evaluation;
pub mod local_laya;
pub mod routing;
pub mod runtime;
pub mod stackbite_plugin;
pub mod workflow;
pub mod adapters;
