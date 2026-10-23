use std::{fs::File, io::Read, sync::Arc};
use switchbite::{
    Plugin,
    contracts::{self, MAX_JSON_BYTES},
    http::HttpJudge,
    mcp_server,
    agent,
    runtime::{DecisionRuntime, RuntimeConfig},
};

fn config(args: &[String]) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let mut client = None;
    let mut executable = None;
    let mut offline = false;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--client" if client.is_none() && index + 1 < args.len() => {
                index += 1;
                client = Some(args[index].as_str());
            }
            "--executable" if executable.is_none() && index + 1 < args.len() => {
                index += 1;
                executable = Some(std::path::PathBuf::from(&args[index]));
            }
            "--offline" if !offline => offline = true,
            _ => return Err("Invalid config arguments; use --help.".into()),
        }
        index += 1;
    }
    let path = executable.unwrap_or(std::env::current_exe()?);
    Ok(switchbite::connection::render(
        client.ok_or("Missing --client; use --help.")?,
        &path,
        offline,
    )?)
}

fn plugin() -> Plugin {
    HttpJudge::from_env()
        .map(Plugin::with_judge)
        .unwrap_or_default()
}
fn request_file(path: &str) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    let mut bytes = Vec::new();
    File::open(path)?.take(MAX_JSON_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    Ok(contracts::parse_request(&bytes)?)
}
fn agent_judge(offline: bool) -> Option<Arc<agent::AgentJudge>> {
    if offline { return None; }
    HttpJudge::from_env().map(|judge| Arc::new(move |body: &serde_json::Value| judge.judge_detailed(body)) as Arc<agent::AgentJudge>)
}
fn agent_config(args: &[String]) -> Result<(bool, RuntimeConfig), Box<dyn std::error::Error + Send + Sync>> {
    let mut config = RuntimeConfig::default();
    let mut offline = false;
    let mut seen = std::collections::BTreeSet::new();
    let mut index = 3;
    while index < args.len() {
        let flag = args[index].as_str();
        if !seen.insert(flag) { return Err("Repeated agent option.".into()); }
        if flag == "--offline" { offline = true; index += 1; continue; }
        let value = args.get(index+1).ok_or("Missing agent option value.")?;
        match flag {
            "--max-concurrent" => config.max_concurrent = value.parse()?,
            "--max-queued" => config.max_queued = value.parse()?,
            "--queue-timeout-ms" => config.queue_timeout_ms = value.parse()?,
            "--decision-timeout-ms" => config.decision_timeout_ms = value.parse()?,
            _ => return Err("Unknown agent option.".into()),
        }
        index += 2;
    }
    // Validate before server startup and before reading a credential.
    let _ = DecisionRuntime::new(config)?;
    Ok((offline, config))
}
fn doctor(client: &str) -> Result<serde_json::Value, switchbite::connection::ConfigError> {
    if ![
        "codex",
        "claude",
        "cursor",
        "opencode",
        "grok",
        "antigravity",
        "generic",
    ]
    .contains(&client)
    {
        return Err(switchbite::connection::ConfigError::UnsupportedClient);
    }
    let names: &[&str] = if client == "opencode" {
        &[
            "OPENCODE_CLI_CONFIG_CONTENT",
            "OPENCODE_CONFIG_CONTENT",
            "OPENCODE_CONFIG",
            "OPENCODE_CONFIG_DIR",
        ]
    } else {
        &[]
    };
    let present: Vec<&str> = names
        .iter()
        .copied()
        .filter(|name| std::env::var_os(name).is_some())
        .collect();
    let mut result = switchbite::cli_tools::status(&plugin());
    result["client"] = serde_json::json!(client);
    result["configuration_overrides_present"] = serde_json::json!(present);
    result["configuration_notice"] = serde_json::json!(
        "Only variable names are shown. OpenCode overrides can take precedence over project files; merge the MCP snippet at the effective config source. This does not test a client connection."
    );
    Ok(result)
}
fn cli(args: &[String]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if args.first().is_some_and(|s| s == "config") {
        print!("{}", config(args)?);
        return Ok(());
    }
    if args.first().is_some_and(|s| s == "setup") {
        let rendered = config(args)?;
        let client = args.windows(2).find(|v| v[0] == "--client").map(|v| v[1].as_str()).ok_or("Missing client.")?;
        println!("{}", serde_json::to_string_pretty(&agent::setup(client, &rendered))?);
        return Ok(());
    }
    let result = match args {
        [command] if command == "guide" => {print!("{}", agent::GUIDE); return Ok(());},
        [command, flag, path] if command == "snapshot" && flag == "--request-file" => {
            switchbite::routing::snapshot(&request_file(path)?, agent::now_ms())?
        }
        [command, flag, path] if command == "evaluate" && flag == "--request-file" => {
            switchbite::evaluation::evaluate(&request_file(path)?)?
        }
        [command, flag, path] if command == "route" && flag == "--request-file" => {
            let request = request_file(path)?;
            let judge = agent_judge(false);
            let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
            let decisions = DecisionRuntime::new(RuntimeConfig::default())?;
            runtime.block_on(decisions.execute(move || agent::route(&request, judge, agent::now_ms())))?
        }
        [command] if command == "describe" => contracts::describe(),
        [command] if command == "doctor" => switchbite::cli_tools::status(&plugin()),
        [command, flag, client] if command == "doctor" && flag == "--client" => doctor(client)?,
        [command, operation, flag, path] if command == "invoke" && flag == "--request-file" => {
            let mut bytes = Vec::new();
            File::open(path)?
                .take(MAX_JSON_BYTES as u64 + 1)
                .read_to_end(&mut bytes)?;
            let request = contracts::parse_request(&bytes)?;
            plugin().invoke(operation, &request)?
        }
        _ => {
            return Err("Usage: switchbite [describe | invoke OP --request-file FILE]".into());
        }
    };
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["--help"] || args == ["-h"] {
        println!(
            "Switchbite — the decision layer for your agents\nStart: setup --client NAME [--offline]\nRecommended MCP: serve --profile agent [--offline]\nAdvanced server options: --max-concurrent N --max-queued N --queue-timeout-ms N --decision-timeout-ms N\nroute | snapshot | evaluate --request-file FILE\nguide | doctor | config --client NAME [--executable ABS] [--offline]\nCompatibility: no args = docking API1; serve --profile cli = original typed tools; invoke OP --request-file FILE"
        );
        return;
    }
    if args.first().is_some_and(|s| s == "serve") && args.get(1).is_some_and(|s| s == "--profile") && args.get(2).is_some_and(|s| s == "agent") {
        let result = (|| -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            let (offline, config) = agent_config(&args)?;
            let judge = agent_judge(offline);
            let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
            runtime.block_on(mcp_server::serve_agent(judge, config, tokio::io::stdin(), tokio::io::stdout()))
        })();
        if result.is_err() { eprintln!("Unable to start agent profile; check --help and bounded runtime settings."); std::process::exit(1); }
        return;
    }
    if args == ["--version"] {
        println!("switchbite {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let serving = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] => Some((mcp_server::Profile::Docking, false)),
        ["serve"] | ["serve", "--profile", "docking"] => {
            Some((mcp_server::Profile::Docking, false))
        }
        ["serve", "--profile", "cli"] => Some((mcp_server::Profile::Cli, false)),
        ["serve", "--profile", "cli", "--offline"] => Some((mcp_server::Profile::Cli, true)),
        _ => None,
    };
    let result = if let Some((profile, offline)) = serving {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|_| "Runtime unavailable.".into())
            .and_then(|runtime| {
                runtime.block_on(mcp_server::serve_with_profile(
                    if offline { Plugin::default() } else { plugin() },
                    tokio::io::stdin(),
                    tokio::io::stdout(),
                    profile,
                ))
            })
    } else {
        cli(&args)
    };
    if let Err(error) = result {
        // Never echo paths, request bytes, credentials, network errors or provider text.
        if let Some(runtime) = error.downcast_ref::<switchbite::runtime::RuntimeError>() {
            // The CLI has the same machine-readable next step as MCP.
            println!("{}", runtime.to_value());
        } else if let Some(domain) = error.downcast_ref::<contracts::ContractError>() {
            eprintln!("{domain}");
        } else if let Some(config) = error.downcast_ref::<switchbite::connection::ConfigError>() {
            eprintln!("{config}");
        } else if args
            .first()
            .is_some_and(|a| a != "invoke" && a != "describe")
        {
            eprintln!("Invalid command or arguments; use switchbite --help.");
        } else {
            eprintln!("Plugin request failed.");
        }
        std::process::exit(1);
    }
}
