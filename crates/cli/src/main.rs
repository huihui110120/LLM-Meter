use clap::{Parser, Subcommand};
use llm_meter_ipc::{Client, call, endpoint};
use serde_json::{Value, json};
use std::{
    io::{BufRead, Write},
    time::Duration,
};

mod diagnostics;
mod platform;
#[cfg(unix)]
#[path = "setup.rs"]
mod setup;
#[cfg(windows)]
#[path = "setup_windows.rs"]
mod setup;
mod update;

#[cfg(not(any(unix, windows)))]
compile_error!("llm-meter CLI currently supports Unix and Windows");

#[derive(Parser)]
#[command(
    name = "llm-meter",
    version,
    about = "Local-first LLM usage and quota monitor"
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Configure platform integration and daemon startup.
    #[command(alias = "install")]
    Setup {
        /// Unix desktop environment to integrate with.
        #[arg(long, value_enum)]
        desktop: Option<setup::DesktopEnvironment>,
        /// Unix top bar to integrate with.
        #[arg(long, value_enum)]
        bar: Option<setup::TopBar>,
        /// Do not prompt; Unix requires explicit --desktop and --bar choices.
        #[arg(long)]
        non_interactive: bool,
        /// Show the selected installation plan without changing files.
        #[arg(long)]
        dry_run: bool,
    },
    /// Update from the platform's supported stable release channel.
    Update,
    /// Inspect or change daemon startup with the user session.
    Autostart {
        #[arg(value_enum)]
        action: AutostartAction,
    },
    #[command(name = "_post-update", hide = true)]
    PostUpdate,
    #[cfg(windows)]
    #[command(name = "_prepare-update", hide = true)]
    PrepareUpdate,
    Status,
    Connections,
    Diagnostics,
    Refresh {
        connection_id: String,
    },
    /// Refresh every configured connection and the local Codex collector.
    RefreshAll,
    Remove {
        connection_id: String,
    },
    Add {
        #[arg(value_enum)]
        kind: AddKind,
        #[arg(long)]
        device: bool,
        /// Open the provider login page in the default browser.
        #[arg(long)]
        open: bool,
        /// Provider to add a connection for.
        #[arg(long, value_enum, default_value_t = Provider::Openai)]
        provider: Provider,
        #[arg(long, default_value = "OpenAI")]
        name: String,
        /// Read the secret credential from one line on standard input.
        #[arg(long)]
        secret_stdin: bool,
        /// Relay profile (required for `add relay`).
        #[arg(long, value_enum)]
        profile: Option<RelayProfile>,
        /// Relay origin URL.
        #[arg(long)]
        base_url: Option<String>,
        /// Local loopback address used by the relay proxy.
        #[arg(long, default_value = "127.0.0.1:18456")]
        listen: String,
    },
    Budget {
        connection_id: String,
        amount: String,
        #[arg(long, default_value = "USD")]
        currency: String,
    },
    Waybar {
        #[arg(long)]
        watch: bool,
    },
    Proxy {
        #[command(subcommand)]
        action: ProxyAction,
    },
    Ui {
        #[arg(long,conflicts_with_all=["hide","main"])]
        toggle: bool,
        #[arg(long)]
        hide: bool,
        #[arg(long)]
        main: bool,
    },
}
#[derive(Clone, Copy, clap::ValueEnum, Default)]
enum Provider {
    #[default]
    Openai,
    Kimi,
    Relay,
}

#[derive(Clone, clap::ValueEnum)]
enum AddKind {
    Subscription,
    Admin,
    Standard,
    Relay,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum RelayProfile {
    NewApi,
    Openrouter,
    Generic,
}

#[derive(Subcommand)]
enum ProxyAction {
    Status {
        connection_id: String,
    },
    Start {
        connection_id: String,
    },
    Stop {
        connection_id: String,
    },
    Credentials {
        connection_id: String,
    },
    CreateCredential {
        connection_id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        token: Option<String>,
    },
    DisableCredential {
        connection_id: String,
        credential_id: String,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum AutostartAction {
    Status,
    Enable,
    Disable,
}
#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("llm-meter: {e}");
        std::process::exit(1)
    }
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let a = Args::parse();
    if let Command::Setup {
        desktop,
        bar,
        non_interactive,
        dry_run,
    } = &a.command
    {
        return setup::run(setup::Options {
            desktop: *desktop,
            bar: *bar,
            non_interactive: *non_interactive,
            dry_run: *dry_run,
        });
    }
    match &a.command {
        Command::Update => return update::run(),
        Command::PostUpdate => return setup::post_update(),
        #[cfg(windows)]
        Command::PrepareUpdate => return setup::prepare_update().await,
        Command::Autostart { action } => {
            let status = match action {
                AutostartAction::Status => setup::autostart_status()?,
                AutostartAction::Enable => setup::set_autostart(true)?,
                AutostartAction::Disable => setup::set_autostart(false)?,
            };
            print(serde_json::to_value(status)?);
            return Ok(());
        }
        Command::Diagnostics => {
            print(diagnostics::collect().await?);
            return Ok(());
        }
        _ => {}
    }
    let p = endpoint()?;
    match a.command {
        Command::Setup { .. } => unreachable!(),
        Command::Update | Command::PostUpdate | Command::Autostart { .. } => unreachable!(),
        #[cfg(windows)]
        Command::PrepareUpdate => unreachable!(),
        Command::Status => print(call(&p, "snapshot/get", json!({})).await?),
        Command::Connections => print(call(&p, "connections/list", json!({})).await?),
        Command::Diagnostics => unreachable!(),
        Command::Refresh { connection_id } => {
            let _: uuid::Uuid = connection_id.parse().map_err(|_| "invalid connection id")?;
            print(
                call(
                    &p,
                    "connections/refresh",
                    json!({"connection_id":connection_id}),
                )
                .await?,
            );
        }
        Command::RefreshAll => {
            let result = call(&p, "connections/refresh-all", json!({})).await?;
            let failed = result
                .get("failed")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            print(result);
            if failed > 0 {
                return Err(format!("{failed} connection(s) could not be refreshed").into());
            }
        }
        Command::Remove { connection_id } => {
            let _: uuid::Uuid = connection_id.parse().map_err(|_| "invalid connection id")?;
            print(
                call(
                    &p,
                    "connections/remove",
                    json!({"connection_id":connection_id}),
                )
                .await?,
            );
        }
        Command::Add {
            kind,
            device,
            open,
            provider,
            name,
            secret_stdin,
            profile,
            base_url,
            listen,
        } => match kind {
            AddKind::Subscription => {
                let (provider_id, connection_type, auth) = match provider {
                    Provider::Kimi => ("kimi", "kimi_code_subscription", "oauth_device_code"),
                    Provider::Relay => return Err("use `llm-meter add relay --profile ...`".into()),
                    Provider::Openai => {
                        if device {
                            ("openai", "chatgpt_subscription", "oauth_device_code")
                        } else {
                            ("openai", "chatgpt_subscription", "oauth_browser")
                        }
                    }
                };
                let challenge = call(
                    &p,
                    "connections/add",
                    json!({"provider_id": provider_id, "connection_type": connection_type, "display_name": name, "auth_scheme": auth}),
                )
                .await?;
                let challenge_id = challenge
                    .get("state")
                    .and_then(Value::as_str)
                    .ok_or("daemon did not return login state")?;
                if let Some(url) = challenge.get("auth_url").and_then(Value::as_str) {
                    eprintln!("Open this URL to authenticate:\n{url}");
                    if open {
                        platform::open_https_url(url)?;
                    }
                }
                if let Some(url) = challenge.get("verification_url").and_then(Value::as_str) {
                    eprintln!(
                        "Open {url} and enter code {}",
                        challenge
                            .get("user_code")
                            .and_then(Value::as_str)
                            .unwrap_or("<missing>")
                    );
                    if open {
                        platform::open_https_url(url)?;
                    }
                }
                print(
                    call(
                        &p,
                        "connections/auth/complete",
                        json!({"challenge_id":challenge_id}),
                    )
                    .await?,
                );
            }
            AddKind::Admin => {
                let challenge=call(&p,"connections/add",json!({"provider_id":"openai","connection_type":"platform_admin","display_name":name,"auth_scheme":"admin_api_key"})).await?;
                let challenge_id = challenge
                    .get("challenge_id")
                    .and_then(Value::as_str)
                    .ok_or("daemon did not return challenge id")?;
                let secret = read_secret(secret_stdin, "OpenAI Admin API Key: ")?;
                let result = call(
                    &p,
                    "connections/auth/complete",
                    json!({"challenge_id":challenge_id,"secret":secret}),
                )
                .await;
                drop(secret);
                print(result?);
            }
            AddKind::Standard => {
                let challenge=call(&p,"connections/add",json!({"provider_id":"openai","connection_type":"platform_standard","display_name":name,"auth_scheme":"api_key"})).await?;
                let challenge_id = challenge
                    .get("challenge_id")
                    .and_then(Value::as_str)
                    .ok_or("daemon did not return challenge id")?;
                let secret = read_secret(secret_stdin, "OpenAI API Key: ")?;
                let result = call(
                    &p,
                    "connections/auth/complete",
                    json!({"challenge_id":challenge_id,"secret":secret}),
                )
                .await;
                drop(secret);
                print(result?);
            }
            AddKind::Relay => {
                let profile = profile.ok_or("--profile is required for relay")?;
                let (connection_type, profile_name, default_origin) = match profile {
                    RelayProfile::NewApi => ("new_api", "new_api", None),
                    RelayProfile::Openrouter => {
                        ("openrouter", "openrouter", Some("https://openrouter.ai"))
                    }
                    RelayProfile::Generic => ("openai_compatible_proxy", "generic", None),
                };
                let origin = base_url
                    .as_deref()
                    .or(default_origin)
                    .ok_or("--base-url is required for this relay profile")?;
                let challenge = call(&p, "connections/add", json!({
                    "provider_id":"relay", "connection_type":connection_type, "display_name":name,
                    "auth_scheme":"api_key", "settings":{"schema_version":1,"values":{
                        "profile":profile_name,"origin":origin,"collection_mode":if matches!(profile, RelayProfile::Generic){"proxy"}else{"pull"},
                        "listen":listen
                    }}
                })).await?;
                let challenge_id = challenge
                    .get("challenge_id")
                    .and_then(Value::as_str)
                    .ok_or("daemon did not return challenge id")?;
                let secret = read_secret(secret_stdin, "Relay API Key: ")?;
                let result = call(
                    &p,
                    "connections/auth/complete",
                    json!({"challenge_id":challenge_id,"secret":secret}),
                )
                .await;
                drop(secret);
                print(result?);
            }
        },
        Command::Proxy { action } => match action {
            ProxyAction::Status { connection_id } => {
                print(call(&p, "proxy/status", json!({"connection_id":connection_id})).await?)
            }
            ProxyAction::Start { connection_id } => {
                print(call(&p, "proxy/start", json!({"connection_id":connection_id})).await?)
            }
            ProxyAction::Stop { connection_id } => {
                print(call(&p, "proxy/stop", json!({"connection_id":connection_id})).await?)
            }
            ProxyAction::Credentials { connection_id } => print(
                call(
                    &p,
                    "proxy/credentials/list",
                    json!({"connection_id":connection_id}),
                )
                .await?,
            ),
            ProxyAction::CreateCredential {
                connection_id,
                name,
                token,
            } => print(
                call(
                    &p,
                    "proxy/credentials/create",
                    json!({"connection_id":connection_id,"display_name":name,"token":token}),
                )
                .await?,
            ),
            ProxyAction::DisableCredential {
                connection_id,
                credential_id,
            } => print(
                call(
                    &p,
                    "proxy/credentials/disable",
                    json!({"connection_id":connection_id,"credential_id":credential_id}),
                )
                .await?,
            ),
        },
        Command::Budget {
            connection_id,
            amount,
            currency,
        } => {
            let _: uuid::Uuid = connection_id.parse().map_err(|_| "invalid connection id")?;
            let _: rust_decimal::Decimal = amount.parse().map_err(|_| "invalid decimal amount")?;
            print(call(&p,"budgets/set",json!({"id":uuid::Uuid::new_v4(),"connection_id":connection_id,"amount":amount,"currency":currency.to_uppercase(),"period":"monthly","warning_ratio":"0.8","critical_ratio":"1","enabled":true})).await?);
        }
        Command::Waybar { watch } => {
            let mut client = None;
            loop {
                if client.is_none() {
                    client = Client::connect(&p).await.ok();
                }
                let result = match client.as_mut() {
                    Some(value) => value.call("waybar/render", json!({})).await,
                    None => Err(std::io::Error::new(
                        std::io::ErrorKind::NotConnected,
                        "daemon offline",
                    )),
                };
                match result {
                    Ok(v) => println!("{}", serde_json::to_string(&v)?),
                    Err(_) => {
                        client = None;
                        println!(
                            "{}",
                            json!({"text":"LLM offline","tooltip":"llm-meter daemon is unavailable","class":["daemon-offline"]})
                        )
                    }
                }
                std::io::stdout().flush()?;
                if !watch {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
        Command::Ui { toggle, hide, main } => {
            let action = if hide {
                "hide"
            } else if main {
                "main"
            } else if toggle {
                "toggle"
            } else {
                "show"
            };
            platform::desktop_command()
                .arg(format!("--{action}"))
                .spawn()
                .map_err(|error| {
                    format!("could not activate llm-meter-desktop ({action}): {error}")
                })?;
        }
    }
    Ok(())
}
fn read_secret(from_stdin: bool, prompt: &str) -> Result<String, Box<dyn std::error::Error>> {
    if !from_stdin {
        return Ok(rpassword::prompt_password(prompt)?);
    }
    read_secret_line(std::io::stdin().lock())
}

fn read_secret_line(mut reader: impl BufRead) -> Result<String, Box<dyn std::error::Error>> {
    let mut secret = String::new();
    reader.read_line(&mut secret)?;
    while matches!(secret.chars().last(), Some('\n' | '\r')) {
        secret.pop();
    }
    if secret.is_empty() {
        return Err("standard input did not contain a secret".into());
    }
    Ok(secret)
}

fn print(v: Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(&v).unwrap_or_else(|_| "null".into())
    )
}
#[cfg(test)]
mod tests {
    use super::read_secret_line;

    #[test]
    fn reads_secret_from_one_line_without_line_ending() {
        let secret = read_secret_line("api-secret\r\ntrailing".as_bytes()).unwrap();
        assert_eq!(secret, "api-secret");
    }

    #[test]
    fn rejects_empty_standard_input_secret() {
        assert!(read_secret_line("\n".as_bytes()).is_err());
    }
}
