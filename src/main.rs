use std::{fs::File, io::Read};
use switchbite::{
    Plugin,
    contracts::{self, MAX_JSON_BYTES},
    http::HttpJudge,
    mcp_server,
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
    let result = match args {
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
            "Switchbite native MCP plugin\nNo args: docking API1 MCP stdio\nserve --profile cli [--offline]: typed tools for CLI agents\ndescribe | doctor | invoke OP --request-file FILE\nconfig --client NAME [--executable ABS] [--offline]"
        );
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
        if let Some(domain) = error.downcast_ref::<contracts::ContractError>() {
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
