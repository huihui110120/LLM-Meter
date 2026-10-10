use std::{io, sync::Arc};

use llm_meter_core::{CORE_VERSION, IPC_VERSION, SCHEMA_VERSION};
use llm_meter_ipc::{Listener, MAX_FRAME_BYTES, encode_frame, read_frame};
use llm_meter_storage::Repository;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::Notify;

pub use llm_meter_ipc::{Client, call};

use crate::scheduler::ProviderRuntime;
use crate::snapshot;

#[derive(Debug, Deserialize)]
struct Request {
    jsonrpc: String,
    #[serde(default)]
    id: Value,
    method: String,
    #[serde(default)]
    params: Value,
}
#[derive(Debug, Serialize)]
struct Response {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}
#[derive(Debug, Serialize)]
struct RpcError {
    code: i32,
    message: String,
}

pub struct Server {
    repo: Arc<Repository>,
    runtime: Option<Arc<ProviderRuntime>>,
    shutdown: Arc<Notify>,
}
impl Server {
    pub fn new(repo: Arc<Repository>) -> Self {
        Self {
            repo,
            runtime: None,
            shutdown: Arc::new(Notify::new()),
        }
    }
    pub fn with_runtime(repo: Arc<Repository>, runtime: Arc<ProviderRuntime>) -> Self {
        Self {
            repo,
            runtime: Some(runtime),
            shutdown: Arc::new(Notify::new()),
        }
    }
    pub async fn serve(self, mut listener: Listener) -> io::Result<()> {
        loop {
            tokio::select! {
                _ = self.shutdown.notified() => return Ok(()),
                accepted = listener.accept() => {
                    let stream = accepted?;
                    let repo = self.repo.clone();
                    let runtime = self.runtime.clone();
                    let shutdown = self.shutdown.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle(stream, repo, runtime, shutdown).await {
                            tracing::warn!(component="ipc",error_code="client_io",error=%e,"IPC client disconnected")
                        }
                    });
                }
            }
        }
    }
}

async fn handle(
    stream: impl AsyncRead + AsyncWrite + Unpin,
    repo: Arc<Repository>,
    runtime: Option<Arc<ProviderRuntime>>,
    shutdown: Arc<Notify>,
) -> io::Result<()> {
    let (read, mut write) = tokio::io::split(stream);
    let mut reader = BufReader::new(read);
    while let Some(line) = read_frame(&mut reader, MAX_FRAME_BYTES).await? {
        let started = std::time::Instant::now();
        let (response, shutdown_requested) = match serde_json::from_slice::<Request>(&line) {
            Ok(r) => {
                let shutdown_requested = r.jsonrpc == "2.0" && r.method == "system/shutdown";
                (
                    dispatch(&repo, runtime.clone(), r).await,
                    shutdown_requested,
                )
            }
            Err(_) => (
                Response {
                    jsonrpc: "2.0",
                    id: Value::Null,
                    result: None,
                    error: Some(RpcError {
                        code: -32700,
                        message: "parse error".into(),
                    }),
                },
                false,
            ),
        };
        crate::telemetry::record_ipc(started.elapsed().as_millis() as u64);
        let bytes = encode_frame(&response, MAX_FRAME_BYTES)?;
        write.write_all(&bytes).await?;
        if shutdown_requested {
            shutdown.notify_one();
            return Ok(());
        }
    }
    Ok(())
}

async fn dispatch(
    repo: &Repository,
    runtime: Option<Arc<ProviderRuntime>>,
    r: Request,
) -> Response {
    let id = r.id;
    if r.jsonrpc != "2.0" {
        return err(id, -32600, "invalid request");
    }
    let result: Result<Value, (i32, &str)> = match r.method.as_str() {
        "system/version" => Ok(
            json!({"core_version":CORE_VERSION,"ipc_version":IPC_VERSION,"schema_version":repo.schema_version().unwrap_or(SCHEMA_VERSION)}),
        ),
        "system/health" => Ok(
            json!({"status":"ok","database":repo.schema_version().is_ok(),"operational_metrics":crate::telemetry::snapshot()}),
        ),
        "system/shutdown" => Ok(json!({"shutting_down":true})),
        "providers/list" => Ok(json!(
            runtime
                .as_ref()
                .map(|v| v.provider_manifests())
                .unwrap_or_default()
        )),
        "connections/list" => repo
            .list_connections()
            .map(|v| json!(v.into_iter().map(public_connection).collect::<Vec<_>>()))
            .map_err(|_| (-32603, "repository error")),
        "connections/capabilities" => param_id(&r.params)
            .and_then(|v| {
                repo.capabilities(v)
                    .map_err(|_| (-32603, "repository error"))
            })
            .map(|v| json!(v)),
        "snapshot/get" => snapshot::build_with_stale_after(
            repo,
            runtime
                .as_ref()
                .map(|v| v.stale_after_seconds())
                .unwrap_or(1800),
        )
        .map(|v| json!(v))
        .map_err(|_| (-32603, "snapshot error")),
        "connections/settings/get" => param_id(&r.params)
            .and_then(|id| {
                repo.connection_settings(id)
                    .map_err(|_| (-32603, "repository error"))
            })
            .map(|value| json!(value)),
        "connections/settings/update" => param_id(&r.params).and_then(|id| {
            let settings = r
                .params
                .get("settings")
                .cloned()
                .ok_or((-32602, "settings required"))
                .and_then(|value| {
                    serde_json::from_value::<llm_meter_core::ConnectionSettings>(value)
                        .map_err(|_| (-32602, "invalid settings"))
                })?;
            repo.update_connection_settings(id, &settings)
                .map_err(|_| (-32603, "repository error"))?;
            Ok(json!(settings))
        }),
        "usage/events" => param_id(&r.params)
            .and_then(|id| {
                repo.usage_events(
                    id,
                    r.params.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize,
                )
                .map_err(|_| (-32603, "repository error"))
            })
            .map(|value| json!(value)),
        "usage/summary" => param_id(&r.params)
            .and_then(|id| {
                repo.metrics(
                    id,
                    None,
                    r.params.get("limit").and_then(Value::as_u64).unwrap_or(500) as usize,
                )
                .map_err(|_| (-32603, "repository error"))
            })
            .map(|value| json!(value)),
        "metrics/query" => param_id(&r.params)
            .and_then(|v| {
                repo.metrics(
                    v,
                    r.params.get("metric_key").and_then(Value::as_str),
                    r.params.get("limit").and_then(Value::as_u64).unwrap_or(500) as usize,
                )
                .map_err(|_| (-32603, "repository error"))
            })
            .map(|v| json!(v)),
        "quotas/list" => param_id(&r.params)
            .and_then(|v| repo.quotas(v).map_err(|_| (-32603, "repository error")))
            .map(|v| json!(v)),
        "waybar/render" => snapshot::build_with_stale_after(
            repo,
            runtime
                .as_ref()
                .map(|v| v.stale_after_seconds())
                .unwrap_or(1800),
        )
        .map(|v| json!(snapshot::render_waybar(&v)))
        .map_err(|_| (-32603, "snapshot error")),
        "budgets/get" => param_id(&r.params)
            .and_then(|id| repo.budget(id).map_err(|_| (-32603, "repository error")))
            .map(|value| json!(value)),
        "budgets/set" => serde_json::from_value::<llm_meter_core::Budget>(r.params.clone())
            .map_err(|_| (-32602, "invalid budget"))
            .and_then(|budget| {
                repo.set_budget(&budget)
                    .map_err(|_| (-32603, "repository error"))?;
                let existing = repo
                    .alerts(Some(budget.connection_id))
                    .map_err(|_| (-32603, "repository error"))?;
                for threshold in [budget.warning_ratio, budget.critical_ratio] {
                    if !existing.iter().any(|v| {
                        v.kind == llm_meter_core::AlertKind::CostBudgetRatio
                            && v.threshold == threshold
                    }) {
                        repo.upsert_alert(&llm_meter_core::AlertRule {
                            id: uuid::Uuid::new_v4(),
                            connection_id: budget.connection_id,
                            kind: llm_meter_core::AlertKind::CostBudgetRatio,
                            threshold,
                            state: llm_meter_core::AlertState::Normal,
                            last_triggered_at: None,
                            suppressed_until: None,
                        })
                        .map_err(|_| (-32603, "repository error"))?;
                    }
                }
                let _ = crate::alerts::evaluate(repo, budget.connection_id);
                Ok(json!(budget))
            }),
        "proxy/status" => match (runtime.as_ref(), param_id(&r.params)) {
            (Some(runtime), Ok(id)) => runtime
                .proxy_running(id)
                .and_then(|running| {
                    runtime
                        .proxy_listen(id)
                        .map(|listen| json!({"running":running,"listen":listen}))
                })
                .map_err(|error| (-32010, error.code())),
            (None, _) => Err((-32001, "provider runtime unavailable")),
            (_, Err(error)) => Err(error),
        },
        "proxy/start" => match (runtime.as_ref(), param_id(&r.params)) {
            (Some(runtime), Ok(id)) => runtime
                .start_proxy(id)
                .await
                .map(|_| json!({"started":true}))
                .map_err(|error| (-32010, error.code())),
            (None, _) => Err((-32001, "provider runtime unavailable")),
            (_, Err(error)) => Err(error),
        },
        "proxy/stop" => match (runtime.as_ref(), param_id(&r.params)) {
            (Some(runtime), Ok(id)) => runtime
                .stop_proxy(id)
                .map(|stopped| json!({"stopped":stopped}))
                .map_err(|error| (-32010, error.code())),
            (None, _) => Err((-32001, "provider runtime unavailable")),
            (_, Err(error)) => Err(error),
        },
        "proxy/credentials/list" => match (runtime.as_ref(), param_id(&r.params)) {
            (Some(runtime), Ok(id)) => runtime
                .proxy_credentials(id)
                .map(|credentials| json!(credentials))
                .map_err(|error| (-32010, error.code())),
            (None, _) => Err((-32001, "provider runtime unavailable")),
            (_, Err(error)) => Err(error),
        },
        "proxy/credentials/create" => match (runtime.as_ref(), param_id(&r.params)) {
            (Some(runtime), Ok(id)) => runtime
                .create_proxy_credential(
                    id,
                    r.params
                        .get("display_name")
                        .and_then(Value::as_str)
                        .unwrap_or("Client"),
                    r.params.get("token").and_then(Value::as_str),
                )
                .map(|(credential, token)| json!({"credential":credential,"token":token}))
                .map_err(|error| (-32010, error.code())),
            (None, _) => Err((-32001, "provider runtime unavailable")),
            (_, Err(error)) => Err(error),
        },
        "proxy/credentials/disable" => match (runtime.as_ref(), param_id(&r.params)) {
            (Some(runtime), Ok(id)) => r
                .params
                .get("credential_id")
                .and_then(Value::as_str)
                .and_then(|value| uuid::Uuid::parse_str(value).ok())
                .ok_or((-32602, "invalid credential_id"))
                .and_then(|credential_id| {
                    runtime
                        .disable_proxy_credential(id, credential_id)
                        .map(|disabled| json!({"disabled":disabled}))
                        .map_err(|error| (-32010, error.code()))
                }),
            (None, _) => Err((-32001, "provider runtime unavailable")),
            (_, Err(error)) => Err(error),
        },
        "alerts/list" => {
            let connection = r
                .params
                .get("connection_id")
                .and_then(Value::as_str)
                .map(uuid::Uuid::parse_str)
                .transpose()
                .map_err(|_| (-32602, "invalid connection_id"));
            connection
                .and_then(|id| repo.alerts(id).map_err(|_| (-32603, "repository error")))
                .map(|value| json!(value))
        }
        "alerts/upsert" => serde_json::from_value::<llm_meter_core::AlertRule>(r.params.clone())
            .map_err(|_| (-32602, "invalid alert"))
            .and_then(|alert| {
                repo.upsert_alert(&alert)
                    .map_err(|_| (-32603, "repository error"))?;
                Ok(json!(alert))
            }),
        "connections/refresh" => match (runtime.as_ref(), param_id(&r.params)) {
            (Some(runtime), Ok(id)) => runtime
                .force_sync(id)
                .await
                .map(|_| json!({"completed":true}))
                .map_err(|e| (-32010, e.code())),
            (None, _) => Err((-32001, "provider runtime unavailable")),
            (_, Err(e)) => Err(e),
        },
        "connections/refresh-all" => match runtime.as_ref() {
            Some(runtime) => {
                let connections = repo
                    .list_connections()
                    .map_err(|_| (-32603, "repository error"));
                match connections {
                    Ok(connections) => {
                        let mut refreshed = Vec::new();
                        let mut failed = Vec::new();
                        for connection in connections {
                            match runtime.force_sync(connection.id).await {
                                Ok(()) => refreshed.push(connection.id.to_string()),
                                Err(error) => failed.push(json!({
                                    "connection_id": connection.id,
                                    "error_code": error.code(),
                                })),
                            }
                        }
                        let _ = tokio::task::spawn_blocking(crate::local_codex::refresh).await;
                        Ok(json!({
                            "completed": true,
                            "refreshed": refreshed,
                            "failed": failed,
                        }))
                    }
                    Err(error) => Err(error),
                }
            }
            None => Err((-32001, "provider runtime unavailable")),
        },
        "connections/remove" => match (runtime.as_deref(), param_id(&r.params)) {
            (Some(runtime), Ok(id)) => runtime
                .remove(id)
                .await
                .map(|_| json!({"removed":true}))
                .map_err(|e| (-32010, e.code())),
            (None, _) => Err((-32001, "provider runtime unavailable")),
            (_, Err(e)) => Err(e),
        },
        "connections/validate-compatible" => match runtime.as_ref() {
            Some(runtime) => {
                let origin = r.params.get("origin").and_then(Value::as_str);
                let secret = r.params.get("secret").and_then(Value::as_str);
                match (origin, secret) {
                    (Some(origin), Some(secret)) => runtime
                        .validate_openai_compatible(
                            origin,
                            secrecy::SecretString::from(secret.to_owned()),
                        )
                        .await
                        .map(|report| json!(report))
                        .map_err(|error| (-32010, error.code())),
                    _ => Err((-32602, "origin and secret required")),
                }
            }
            None => Err((-32001, "provider runtime unavailable")),
        },
        "connections/add" => match runtime.as_deref() {
            Some(runtime) => {
                let provider = r.params.get("provider_id").and_then(Value::as_str);
                let request =
                    serde_json::from_value::<llm_meter_core::BeginAuthRequest>(r.params.clone());
                match (provider, request) {
                    (Some(provider), Ok(request)) => runtime
                        .begin_auth(request, provider)
                        .await
                        .map(|v| json!(v))
                        .map_err(|e| (-32010, e.code())),
                    _ => Err((-32602, "invalid connection auth request")),
                }
            }
            None => Err((-32001, "provider runtime unavailable")),
        },
        "connections/auth/complete" => match runtime.as_ref() {
            Some(runtime) => {
                let challenge = r.params.get("challenge_id").and_then(Value::as_str);
                let secret = r
                    .params
                    .get("secret")
                    .and_then(Value::as_str)
                    .map(|v| secrecy::SecretString::from(v.to_owned()));
                match challenge {
                    Some(id) => match runtime.complete_auth(id, secret).await {
                        Ok(connection) => {
                            // Authentication success should make useful data available without a
                            // second user action. The sync stays asynchronous so the auth response
                            // can update the UI immediately.
                            let _ = runtime.request_sync(connection.id, false);
                            if connection.connection_type == "openai_compatible_proxy" {
                                match runtime.start_proxy(connection.id).await {
                                    Ok(()) => Ok(json!(public_connection(connection))),
                                    Err(error) => Err((-32010, error.code())),
                                }
                            } else {
                                Ok(json!(public_connection(connection)))
                            }
                        }
                        Err(error) => Err((-32010, error.code())),
                    },
                    None => Err((-32602, "challenge_id required")),
                }
            }
            None => Err((-32001, "provider runtime unavailable")),
        },
        _ => Err((-32601, "method not found")),
    };
    match result {
        Ok(v) => Response {
            jsonrpc: "2.0",
            id,
            result: Some(v),
            error: None,
        },
        Err((c, m)) => err(id, c, m),
    }
}
fn param_id(v: &Value) -> Result<uuid::Uuid, (i32, &'static str)> {
    v.get("connection_id")
        .and_then(Value::as_str)
        .ok_or((-32602, "connection_id required"))
        .and_then(|v| uuid::Uuid::parse_str(v).map_err(|_| (-32602, "invalid connection_id")))
}
fn err(id: Value, code: i32, message: &str) -> Response {
    Response {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(RpcError {
            code,
            message: message.into(),
        }),
    }
}
fn public_connection(c: llm_meter_core::Connection) -> Value {
    json!({"id":c.id,"provider_id":c.provider_id,"connection_type":c.connection_type,"display_name":c.display_name,"account_external_id":c.account_external_id,"status":c.status,"created_at":c.created_at,"updated_at":c.updated_at,"last_success_at":c.last_success_at,"last_error_code":c.last_error_code,"disabled_at":c.disabled_at})
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use llm_meter_core::{Connection, ConnectionStatus, CredentialRef};
    use llm_meter_ipc::IpcEndpoint;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use uuid::Uuid;
    #[tokio::test]
    async fn private_socket_and_allowlisted_connection_dto() {
        let root = std::env::temp_dir().join(format!("llm-meter-ipc-{}", Uuid::new_v4()));
        #[cfg(unix)]
        let endpoint = IpcEndpoint::from_path(root.join("daemon.sock"));
        #[cfg(windows)]
        let endpoint = IpcEndpoint::from_pipe_name(format!(
            r"\\.\pipe\io.github.llmmeter.test.{}",
            Uuid::new_v4()
        ))
        .unwrap();
        let repo = Arc::new(Repository::in_memory().unwrap());
        let credential = CredentialRef {
            id: Uuid::new_v4(),
            backend: "memory".into(),
            service_name: "secret-service".into(),
            secret_key: "must-not-leak".into(),
            created_at: Utc::now(),
        };
        let now = Utc::now();
        let connection = Connection {
            id: Uuid::new_v4(),
            provider_id: "mock".into(),
            connection_type: "fixture".into(),
            display_name: "Mock".into(),
            account_external_id: None,
            status: ConnectionStatus::Ready,
            credential_ref_id: Some(credential.id),
            created_at: now,
            updated_at: now,
            last_success_at: None,
            last_error_code: None,
            disabled_at: None,
        };
        repo.add_authenticated_connection(&connection, Some(&credential), None)
            .unwrap();
        let listener = Listener::bind(&endpoint).await.unwrap();
        let server = Server::new(repo);
        let task = tokio::spawn(async move { server.serve(listener).await });
        #[cfg(unix)]
        {
            assert_eq!(
                std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(endpoint.as_path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let list = call(&endpoint, "connections/list", json!({}))
            .await
            .unwrap();
        assert!(list[0].get("credential_ref_id").is_none());
        assert!(!list.to_string().contains("must-not-leak"));
        assert_eq!(
            call(&endpoint, "system/shutdown", json!({})).await.unwrap(),
            json!({"shutting_down":true})
        );
        tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .expect("server did not stop after the shutdown request")
            .unwrap()
            .unwrap();
        let _ = std::fs::remove_dir_all(root);
    }
}
