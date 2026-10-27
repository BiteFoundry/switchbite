use std::{fs::File, io::Read, sync::Arc};
use switchbite::{
    Plugin, agent,
    contracts::{self, MAX_JSON_BYTES},
    http::HttpJudge,
    local_laya::{DEFAULT_ENDPOINT, DEFAULT_MODEL, LayaConfig, LocalLayaJudge},
    mcp_server,
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

#[derive(Default)]
enum JudgeSelection {
    #[default]
    Jev,
    Laya(LayaConfig),
}

fn judge_selection(
    args: &mut Vec<String>,
) -> Result<JudgeSelection, switchbite::runtime::RuntimeError> {
    let mut judge = None;
    let mut endpoint = None;
    let mut model = None;
    let mut kept = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let slot = match args[i].as_str() {
            "--judge" => Some(&mut judge),
            "--endpoint" => Some(&mut endpoint),
            "--model" => Some(&mut model),
            _ => None,
        };
        if let Some(slot) = slot {
            if slot.is_some() || i + 1 >= args.len() {
                return Err(switchbite::runtime::RuntimeError::from_code(
                    "local_judge_config",
                ));
            }
            *slot = Some(args[i + 1].clone());
            i += 2;
        } else {
            kept.push(args[i].clone());
            i += 1;
        }
    }
    *args = kept;
    match judge.as_deref() {
        Some("laya") => Ok(JudgeSelection::Laya(LayaConfig::new(
            endpoint.as_deref().unwrap_or(DEFAULT_ENDPOINT),
            model.as_deref().unwrap_or(DEFAULT_MODEL),
        )?)),
        Some("jev") | None if endpoint.is_none() && model.is_none() => Ok(JudgeSelection::Jev),
        _ => Err(switchbite::runtime::RuntimeError::from_code(
            "local_judge_config",
        )),
    }
}
fn plugin(selection: &JudgeSelection) -> Plugin {
    match selection {
        JudgeSelection::Jev => HttpJudge::from_env()
            .map(Plugin::with_judge)
            .unwrap_or_default(),
        JudgeSelection::Laya(config) => Plugin::with_judge(LocalLayaJudge::new(config.clone())),
    }
}
fn request_file(path: &str) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_JSON_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    Ok(contracts::parse_request(&bytes)?)
}
fn agent_judge(selection: &JudgeSelection, offline: bool) -> Option<Arc<agent::ConfiguredJudge>> {
    if offline {
        return None;
    }
    match selection {
        JudgeSelection::Jev => HttpJudge::from_env().map(|judge| {
            Arc::new(agent::ConfiguredJudge::legacy(Arc::new(move |body| {
                judge.judge_detailed(body)
            })))
        }),
        JudgeSelection::Laya(config) => {
            let model = config.model().to_owned();
            let judge = LocalLayaJudge::new(config.clone());
            Some(Arc::new(
                agent::ConfiguredJudge::new(
                    model,
                    Arc::new(move |body| judge.judge_detailed(body)),
                )
                .expect("validated Laya identity"),
            ))
        }
    }
}
fn agent_config(
    args: &[String],
) -> Result<(bool, RuntimeConfig), Box<dyn std::error::Error + Send + Sync>> {
    let mut config = RuntimeConfig::default();
    let mut offline = false;
    let mut seen = std::collections::BTreeSet::new();
    let mut index = 3;
    while index < args.len() {
        let flag = args[index].as_str();
        if !seen.insert(flag) {
            return Err("Repeated agent option.".into());
        }
        if flag == "--offline" {
            offline = true;
            index += 1;
            continue;
        }
        let value = args.get(index + 1).ok_or("Missing agent option value.")?;
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
fn doctor(
    client: &str,
    selection: &JudgeSelection,
) -> Result<serde_json::Value, switchbite::connection::ConfigError> {
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
    let configured = agent_judge(selection, false);
    let mut result = agent::configured_status(configured.as_deref());
    result["client"] = serde_json::json!(client);
    result["configuration_overrides_present"] = serde_json::json!(present);
    result["configuration_notice"] = serde_json::json!(
        "Only variable names are shown. OpenCode overrides can take precedence over project files; merge the MCP snippet at the effective config source. This does not test a client connection."
    );
    Ok(result)
}
fn cli(
    args: &[String],
    selection: &JudgeSelection,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if args
        .first()
        .is_some_and(|s| switchbite::connect::CLIENTS.contains(&s.as_str()))
    {
        if args.len() == 2 && matches!(args[1].as_str(), "--help" | "-h") {
            println!(
                "Usage: switchbite {} [--dry-run] [--json] [--offline] [--config-file FILE]\nConnects this user once. Start a new client session afterward; judge settings stay separate.",
                args[0]
            );
            return Ok(());
        }
        let client = &args[0];
        let mut dry_run = false;
        let mut json = false;
        let mut offline = false;
        let mut file = None;
        let mut seen = std::collections::BTreeSet::new();
        let mut index = 1;
        while index < args.len() {
            let flag = args[index].as_str();
            if !seen.insert(flag) {
                return Err("Repeated connection option.".into());
            }
            match flag {
                "--dry-run" => dry_run = true,
                "--json" => json = true,
                "--offline" => offline = true,
                "--config-file" => {
                    index += 1;
                    file = Some(std::path::PathBuf::from(
                        args.get(index).ok_or("Missing config path.")?,
                    ));
                }
                _ => return Err("Unknown connection option.".into()),
            }
            index += 1;
        }
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .map(std::path::PathBuf::from)
            .ok_or(switchbite::connect::ConnectError::Environment)?;
        let configuration = match file {
            Some(path) if path.is_absolute() => path,
            Some(path) => std::env::current_dir()?.join(path),
            None => switchbite::connect::config_path(client, &home)?,
        };
        let data = std::env::var_os(if cfg!(windows) {
            "LOCALAPPDATA"
        } else {
            "XDG_DATA_HOME"
        })
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
        if !data.is_absolute() {
            return Err(switchbite::connect::ConnectError::Environment.into());
        }
        let source = std::env::current_exe()?;
        let mut executable = switchbite::connect::managed_executable(&source, &data, true)?;
        // Validate the effective configuration before making even a binary copy.
        switchbite::connect::connect_to(client, &configuration, &executable, offline, true)?;
        if !dry_run {
            // An updater may have replaced the source since the preview read.
            // Register the copy actually installed, rather than the preview hash.
            executable = switchbite::connect::managed_executable(&source, &data, false)?;
        }
        let report =
            switchbite::connect::connect_to(client, &configuration, &executable, offline, dry_run)?;
        if json {
            println!("{}", serde_json::to_string(&report)?);
        } else {
            println!(
                "Switchbite → {client}: {}.\nConfiguration: {}",
                report.status,
                report.configuration.display()
            );
            if dry_run {
                println!("Preview only. Run without --dry-run to connect.");
            } else {
                println!(
                    "Open a new {client} session. Approve Switchbite once if the client asks.\nRouting instructions load automatically; judge selection and credentials are unchanged."
                );
            }
        }
        return Ok(());
    }
    if args.first().is_some_and(|s| s == "config") {
        print!("{}", config(args)?);
        return Ok(());
    }
    if args.first().is_some_and(|s| s == "setup") {
        let rendered = config(args)?;
        let client = args
            .windows(2)
            .find(|v| v[0] == "--client")
            .map(|v| v[1].as_str())
            .ok_or("Missing client.")?;
        println!(
            "{}",
            serde_json::to_string_pretty(&agent::setup(client, &rendered))?
        );
        return Ok(());
    }
    if args.first().is_some_and(|s| s == "models") {
        let refresh = match args {
            [_] => false,
            [_, flag] if flag == "--refresh" => true,
            _ => return Err("Usage: switchbite models [--refresh]".into()),
        };
        match switchbite::catalog::snapshot(
            &std::env::current_dir()?,
            &serde_json::json!({"refresh":refresh}),
        ) {
            Ok(snapshot) => println!("{}", serde_json::to_string_pretty(&snapshot)?),
            Err(error) => {
                println!("{}", error.to_value());
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    if args.first().is_some_and(|s| s == "workflow") {
        let workspace = std::env::current_dir()?;
        let root = workspace.join(".switchbite").join("workflow");
        if let [_, command, flag, path] = args
            && command == "run"
            && flag == "--request-file"
        {
            let request = request_file(path)?;
            match switchbite::workflow_runner::run_to_completion(
                &request,
                &root,
                agent_judge(selection, false),
            ) {
                Ok(result) => println!("{result}"),
                Err(error) => {
                    println!("{}", error.to_value());
                    std::process::exit(1);
                }
            }
            return Ok(());
        }
        if let [_, command, run] = args {
            if command == "continue" {
                match switchbite::workflow_runner::resume_to_completion(
                    run,
                    &root,
                    agent_judge(selection, false),
                ) {
                    Ok(result) => println!("{result}"),
                    Err(error) => {
                        println!("{}", error.to_value());
                        std::process::exit(1);
                    }
                }
                return Ok(());
            }
            if command == "view" {
                return switchbite::workflow_ui::serve(&workspace, run, 58420);
            }
        }
        if let [_, command, run, flag, port] = args
            && command == "view"
            && flag == "--port"
        {
            return switchbite::workflow_ui::serve(&workspace, run, port.parse()?);
        }
        let interface = switchbite::workflow_interface::Interface::new(
            workspace,
            agent_judge(selection, false),
        );
        let result = match args {
            [_, command, flag, path] if command == "start" && flag == "--request-file" => {
                interface.start(&request_file(path)?)
            }
            [_, command, run, flag, path] if command == "observe" && flag == "--request-file" => {
                interface.observe(run, &request_file(path)?)
            }
            [_, command, run] if command == "next" => {
                let engine = switchbite::workflow::Workflow::new(root.clone());
                match engine.request(run) {
                    Ok(request) if switchbite::workflow_runner::local_execution(&request) => {
                        switchbite::workflow_runner::resume_to_completion(
                            run,
                            &root,
                            agent_judge(selection, false),
                        )
                        .map_err(|e| e.to_value())
                    }
                    Ok(_) => interface.next(run),
                    Err(e) => Err(e.to_value()),
                }
            }
            [_, command, run] if command == "status" => interface.status(run),
            [_, command, run] if matches!(command.as_str(), "pause" | "resume" | "cancel") => {
                match interface.control(run, command) {
                    Ok(controlled) => {
                        let engine = switchbite::workflow::Workflow::new(root.clone());
                        if command != "pause"
                            && engine.request(run).is_ok_and(|request| {
                                switchbite::workflow_runner::local_execution(&request)
                            })
                        {
                            switchbite::workflow_runner::resume_to_completion(
                                run,
                                &root,
                                agent_judge(selection, false),
                            )
                            .map_err(|e| e.to_value())
                        } else {
                            Ok(controlled)
                        }
                    }
                    Err(error) => Err(error),
                }
            }
            _ => Err(switchbite::workflow_interface::invalid()),
        };
        match result {
            Ok(value) => println!("{}", value),
            Err(error) => {
                println!("{}", error);
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    let result = match args {
        [command] if command == "guide" => {
            print!("{}", agent::GUIDE);
            return Ok(());
        }
        [command, flag, path] if command == "snapshot" && flag == "--request-file" => {
            switchbite::routing::snapshot(&request_file(path)?, agent::now_ms())?
        }
        [command, flag, path] if command == "evaluate" && flag == "--request-file" => {
            switchbite::evaluation::evaluate(&request_file(path)?)?
        }
        [command, flag, path] if command == "route" && flag == "--request-file" => {
            let request = request_file(path)?;
            let judge = agent_judge(selection, false);
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            let decisions = DecisionRuntime::new(RuntimeConfig::default())?;
            runtime.block_on(
                decisions
                    .execute(move || agent::route_with_judge(&request, judge, agent::now_ms())),
            )?
        }
        [command] if command == "describe" => contracts::describe(),
        [command] if command == "doctor" => {
            agent::configured_status(agent_judge(selection, false).as_deref())
        }
        [command, flag, client] if command == "doctor" && flag == "--client" => {
            doctor(client, selection)?
        }
        [command] if command == "probe" => match selection {
            JudgeSelection::Laya(config) => LocalLayaJudge::new(config.clone()).probe()?,
            JudgeSelection::Jev => return Err("probe requires explicit --judge laya".into()),
        },
        [command, operation, flag, path] if command == "invoke" && flag == "--request-file" => {
            let mut bytes = Vec::new();
            File::open(path)?
                .take(MAX_JSON_BYTES as u64 + 1)
                .read_to_end(&mut bytes)?;
            let request = contracts::parse_request(&bytes)?;
            plugin(selection).invoke(operation, &request)?
        }
        _ => {
            return Err("Usage: switchbite [describe | invoke OP --request-file FILE]".into());
        }
    };
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
fn install_stackbite(args: &[String]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use switchbite::stackbite_plugin::{self, InstallOptions, JudgeConfig};
    if args.len() == 2 && matches!(args[1].as_str(), "--help" | "-h") {
        println!(
            "Usage: switchbite stackbite [--workspace ABS] [--client codex|claude|both] [--judge jev|laya] [--endpoint URL --model NAME] [--dry-run] [--json]\nDefault judge: Jev 1.14 (TYPESAFE_API_KEY in the coordinator environment). Endpoint/model options require --judge laya. Installs only project MCP settings as switchbite-stackbite; start a new trusted coordinator session afterward."
        );
        return Ok(());
    }
    let mut options = InstallOptions {
        workspace: std::env::current_dir()?,
        client: "both".into(),
        judge: JudgeConfig::Jev,
        dry_run: false,
    };
    let mut json_output = false;
    let mut selected_judge = "jev";
    let mut endpoint = None;
    let mut model = None;
    let mut seen = std::collections::BTreeSet::new();
    let mut i = 1;
    while i < args.len() {
        if !seen.insert(args[i].clone()) {
            return Err("Repeated install option.".into());
        }
        match args[i].as_str() {
            "--dry-run" => options.dry_run = true,
            "--json" => json_output = true,
            name @ ("--workspace" | "--client" | "--endpoint" | "--model" | "--judge") => {
                let value = args.get(i + 1).ok_or("Missing install option value.")?;
                match name {
                    "--workspace" => options.workspace = value.into(),
                    "--client" => options.client = value.clone(),
                    "--endpoint" => endpoint = Some(value.clone()),
                    "--model" => model = Some(value.clone()),
                    "--judge" if matches!(value.as_str(), "jev" | "laya") => selected_judge = value,
                    _ => return Err("Stackbite judge must be jev or laya.".into()),
                };
                i += 1;
            }
            _ => return Err("Unknown install option.".into()),
        }
        i += 1;
    }
    options.judge = match selected_judge {
        "laya" => JudgeConfig::Laya {
            endpoint: endpoint.unwrap_or_else(|| DEFAULT_ENDPOINT.into()),
            model: model.unwrap_or_else(|| DEFAULT_MODEL.into()),
        },
        _ if endpoint.is_none() && model.is_none() => JudgeConfig::Jev,
        _ => return Err(
            "--endpoint and --model require --judge laya; Jev uses its fixed service and model."
                .into(),
        ),
    };
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(std::path::PathBuf::from)
        .ok_or("Missing user directory.")?;
    let data = std::env::var_os(if cfg!(windows) {
        "LOCALAPPDATA"
    } else {
        "XDG_DATA_HOME"
    })
    .map(std::path::PathBuf::from)
    .unwrap_or_else(|| home.join(".local/share"));
    match stackbite_plugin::install(&options, &std::env::current_exe()?, &data) {
        Ok(report) => {
            if json_output {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                println!(
                    "Switchbite ({}) → Stackbite coordinator: {}\nWorkspace: {}\nServer: {}\n{}",
                    report.judge["kind"].as_str().unwrap_or("unknown"),
                    report.status,
                    report.workspace.display(),
                    report.server,
                    report.next_action
                );
            }
        }
        Err(error) => {
            if json_output {
                println!("{}", serde_json::to_string(&error)?);
            } else {
                eprintln!("{error}");
            }
            std::process::exit(1);
        }
    }
    Ok(())
}
fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|s| s == "stackbite") {
        if let Err(error) = install_stackbite(&args) {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    let selection = match judge_selection(&mut args) {
        Ok(selection) => selection,
        Err(error) => {
            println!("{}", error.to_value());
            std::process::exit(1);
        }
    };
    if matches!(selection, JudgeSelection::Laya(_))
        && !matches!(
            args.first().map(String::as_str),
            Some(
                "serve"
                    | "workflow"
                    | "route"
                    | "invoke"
                    | "doctor"
                    | "probe"
                    | "--help"
                    | "-h"
                    | "--version"
            )
        )
    {
        eprintln!(
            "Use switchbite stackbite to install local Laya, or select it on serve, workflow, route, invoke, doctor or probe."
        );
        std::process::exit(1);
    }
    if args == ["--help"] || args == ["-h"] {
        println!(
            "Switchbite — the decision layer for your agents\nConnect once: switchbite codex | claude | cursor | grok | antigravity | opencode\nConnection options: --dry-run --json --offline --config-file FILE\nCopies a stable executable and merges user-level MCP settings; judge choice stays separate.\nPrint-only setup: setup --client NAME [--offline]\nModels: models [--refresh]\nWorkflow MCP: serve --profile workflow [--workspace DIR] [--offline]\nWorkflow CLI: workflow run|start --request-file FILE\nworkflow next|continue|status|pause|resume|cancel RUN\nworkflow observe RUN --request-file FILE | workflow view RUN [--port PORT]\nLegacy MCP: serve --profile agent [--offline]\nLocal Laya: --judge laya [--endpoint http://127.0.0.1:11435] [--model laya:multilingual]\nStackbite plugin: stackbite [--workspace ABS] [--client codex|claude|both] [--judge jev|laya] [--endpoint URL] [--model NAME] [--dry-run] [--json] (default Jev; project server switchbite-stackbite)\nLocal synthetic check: probe --judge laya [--endpoint URL] [--model NAME]\nAdvanced server options: --max-concurrent N --max-queued N --queue-timeout-ms N --decision-timeout-ms N\nroute | snapshot | evaluate --request-file FILE\nguide | doctor | config --client NAME [--executable ABS] [--offline]\nCompatibility: no args = docking API1; serve --profile cli = original typed tools; invoke OP --request-file FILE"
        );
        return;
    }
    if args.first().is_some_and(|s| s == "serve")
        && args.get(1).is_some_and(|s| s == "--profile")
        && args.get(2).is_some_and(|s| s == "workflow")
    {
        let result = (|| -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            let mut options = args.clone();
            let mut workspace = None;
            if let Some(index) = options.iter().position(|s| s == "--workspace") {
                if index + 1 >= options.len() {
                    return Err("Missing workspace.".into());
                }
                workspace = Some(std::path::PathBuf::from(options.remove(index + 1)));
                options.remove(index);
            }
            let (offline, config) = agent_config(&options)?;
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            runtime.block_on(mcp_server::serve_workflow(
                workspace,
                agent_judge(&selection, offline),
                config,
                tokio::io::stdin(),
                tokio::io::stdout(),
            ))
        })();
        if result.is_err() {
            eprintln!("Unable to start workflow profile; check --help and options.");
            std::process::exit(1);
        }
        return;
    }
    if args.first().is_some_and(|s| s == "serve")
        && args.get(1).is_some_and(|s| s == "--profile")
        && args.get(2).is_some_and(|s| s == "agent")
    {
        let result = (|| -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            let (offline, config) = agent_config(&args)?;
            let judge = agent_judge(&selection, offline);
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            runtime.block_on(mcp_server::serve_agent_with_judge(
                judge,
                config,
                tokio::io::stdin(),
                tokio::io::stdout(),
            ))
        })();
        if result.is_err() {
            eprintln!("Unable to start agent profile; check --help and bounded runtime settings.");
            std::process::exit(1);
        }
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
                    if offline {
                        Plugin::default()
                    } else {
                        plugin(&selection)
                    },
                    tokio::io::stdin(),
                    tokio::io::stdout(),
                    profile,
                ))
            })
    } else {
        cli(&args, &selection)
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
        } else if let Some(connect) = error.downcast_ref::<switchbite::connect::ConnectError>() {
            eprintln!("{connect}");
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
