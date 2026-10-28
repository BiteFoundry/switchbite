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

pub mod activity;
pub mod adapters;
pub mod agent;
pub mod catalog;
pub mod engine_updates;
pub mod evaluation;
pub mod local_laya;
pub mod route_context;
pub mod routing;
pub mod runtime;
pub mod runtime_bundle;
pub mod stackbite_plugin;
pub mod web_coordinator;
pub mod web_workbench;
pub mod workflow;
mod workflow_artifacts;
pub mod workflow_interface;
pub mod workflow_runner;
pub mod workflow_ui;

pub mod worker_feedback;
