//! Generic dispatch of module operations for the `run_operation` and
//! `list_modules` IPC commands. No Tauri dependency.
//!
//! The frontend can only name a `(module, operation)` pair that is compiled
//! into the app's [`Registry`]. It cannot supply a program, argv, shell
//! string, path, URL or privilege level, and no [`ossiframe_core::Executor`]
//! functionality is exposed. `args` is opaque JSON that the module validates
//! in Rust.
//!
//! All S02 operations are read-only because the included modules use only
//! predefined read commands, not because this dispatcher enforces it. S04
//! must add explicit authorization and approval boundaries before any
//! mutating or privileged operation exists.

use crate::ipc::{IpcError, cap};
use crate::service::ServerService;
use ossiframe_core::{
    ModuleId, ModuleManifest, OperationContext, OperationError, OperationId, OperationInfo,
    Registry, RunError, ValidationError,
};
use serde::Serialize;

/// Largest accepted serialized `args`.
pub const MAX_ARGS_BYTES: usize = 16 * 1024;
/// Largest accepted serialized operation result.
pub const MAX_RESULT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationDto {
    pub id: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModuleDto {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub dependencies: Vec<String>,
    pub operations: Vec<OperationDto>,
}

fn module_dto(manifest: ModuleManifest, ops: &[OperationInfo]) -> ModuleDto {
    ModuleDto {
        id: manifest.id.to_string(),
        name: manifest.name,
        version: manifest.version,
        description: manifest.description,
        dependencies: manifest
            .dependencies
            .iter()
            .map(|d| d.to_string())
            .collect(),
        operations: ops
            .iter()
            .map(|o| OperationDto {
                id: o.id.to_owned(),
                description: o.description.to_owned(),
            })
            .collect(),
    }
}

/// Registered modules and their operations, in resolved order.
pub fn list_modules(registry: &Registry) -> Vec<ModuleDto> {
    registry
        .describe()
        .into_iter()
        .map(|(m, ops)| module_dto(m, ops))
        .collect()
}

/// Inputs that passed every check that does not need a connection.
struct Checked {
    server_id: String,
    module: ModuleId,
    operation: OperationId,
}

/// Rejects bad input and unknown module or operation names before any
/// connection is touched.
fn check(
    registry: &Registry,
    server_id: &str,
    module_id: &str,
    operation_id: &str,
    args: &serde_json::Value,
) -> Result<Checked, IpcError> {
    let canonical = uuid::Uuid::parse_str(server_id)
        .map(|u| u.to_string())
        .map_err(|_| IpcError::NotFound)?;
    if canonical != server_id {
        return Err(IpcError::NotFound);
    }
    let module = ModuleId::parse(module_id).map_err(|e: ValidationError| IpcError::from(e))?;
    let operation = OperationId::parse(operation_id).map_err(IpcError::from)?;
    let size = serde_json::to_vec(args)
        .map(|v| v.len())
        .unwrap_or(usize::MAX);
    if size > MAX_ARGS_BYTES {
        return Err(IpcError::Validation {
            field: "args".into(),
            message: format!("must be at most {MAX_ARGS_BYTES} bytes"),
        });
    }
    if !registry.has_operation(&module, &operation) {
        return Err(IpcError::NotFound);
    }
    Ok(Checked {
        server_id: server_id.to_owned(),
        module,
        operation,
    })
}

fn observed_now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn map_run_error(e: RunError) -> IpcError {
    match e {
        RunError::UnknownModule(_) | RunError::UnknownOperation { .. } => IpcError::NotFound,
        RunError::Operation(OperationError::InvalidArguments(m)) => IpcError::Validation {
            field: "args".into(),
            message: cap(&m),
        },
        RunError::Operation(OperationError::Exec(e)) => e.into(),
        // Short, capped message. Never raw output.
        RunError::Operation(OperationError::Failed(m)) => {
            IpcError::OperationFailed { message: cap(&m) }
        }
    }
}

/// Runs one operation of a registered module on the server's live connection.
pub async fn run_operation(
    service: &ServerService,
    registry: &Registry,
    server_id: &str,
    module_id: &str,
    operation_id: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, IpcError> {
    let checked = check(registry, server_id, module_id, operation_id, &args)?;
    let observed_at = observed_now();
    let Checked {
        server_id,
        module,
        operation,
    } = checked;
    let result = service
        .with_connection(&server_id, |conn, cancel| async move {
            let ctx = OperationContext {
                exec: &*conn,
                cancel: &cancel,
                observed_at,
            };
            registry.run(&module, &operation, args, &ctx).await
        })
        .await?
        .map_err(map_run_error)?;
    let size = serde_json::to_vec(&result)
        .map(|v| v.len())
        .unwrap_or(usize::MAX);
    if size > MAX_RESULT_BYTES {
        return Err(IpcError::OperationFailed {
            message: "result too large".into(),
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::Connector;
    use crate::session::{Connection, NoEvents, tests::FakeConn};
    use crate::store::{AuthConfig, Store};
    use crate::vault::MemoryVault;
    use async_trait::async_trait;
    use ossiframe_core::host_key::PinnedHostKey;
    use ossiframe_core::{Hostname, Module, Port, ServerName, SshUser};
    use ossiframe_ssh::{AuthMethod, ConnectError, ConnectTarget};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static OPS: &[OperationInfo] = &[OperationInfo {
        id: "ping",
        description: "test",
    }];

    struct Probe {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Module for Probe {
        fn manifest(&self) -> ModuleManifest {
            ModuleManifest {
                id: ModuleId::parse("probe").unwrap(),
                name: "Probe".into(),
                version: "0.0.0".into(),
                description: String::new(),
                dependencies: Vec::new(),
            }
        }
        fn operations(&self) -> &'static [OperationInfo] {
            OPS
        }
        async fn run(
            &self,
            _: &str,
            args: serde_json::Value,
            ctx: &OperationContext<'_>,
        ) -> Result<serde_json::Value, OperationError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if args.get("big").is_some() {
                return Ok(serde_json::json!("x".repeat(MAX_RESULT_BYTES)));
            }
            if args.get("bad").is_some() {
                return Err(OperationError::InvalidArguments("nope".into()));
            }
            Ok(serde_json::json!({ "observedAt": ctx.observed_at }))
        }
    }

    struct FakeConnector;

    #[async_trait]
    impl Connector for FakeConnector {
        async fn connect(
            &self,
            _: &ConnectTarget,
            _: &AuthMethod,
            _: Option<PinnedHostKey>,
        ) -> Result<Arc<dyn Connection>, ConnectError> {
            Ok(Arc::new(FakeConn::default()))
        }
    }

    fn setup() -> (ServerService, Registry, Arc<AtomicUsize>, String) {
        let svc = ServerService::new(
            Store::open_in_memory().unwrap(),
            Arc::new(MemoryVault::default()),
            Arc::new(FakeConnector),
            Arc::new(NoEvents),
        );
        let id = svc
            .add(crate::service::ProfileInput {
                name: ServerName::parse("web").unwrap(),
                host: Hostname::parse("example.com").unwrap(),
                port: Port::SSH_DEFAULT,
                username: SshUser::parse("root").unwrap(),
                auth: AuthConfig::Password,
            })
            .unwrap()
            .record
            .id;
        let calls = Arc::new(AtomicUsize::new(0));
        let registry = Registry::builder()
            .register(Probe {
                calls: calls.clone(),
            })
            .build()
            .unwrap();
        (svc, registry, calls, id)
    }

    fn null() -> serde_json::Value {
        serde_json::Value::Null
    }

    #[tokio::test]
    async fn bad_input_is_rejected_before_the_connection_is_used() {
        let (svc, reg, calls, id) = setup();
        let run = |s: &str, m: &str, o: &str, a: serde_json::Value| {
            let (svc, reg, s, m, o) = (&svc, &reg, s.to_owned(), m.to_owned(), o.to_owned());
            async move { run_operation(svc, reg, &s, &m, &o, a).await }
        };
        // A known operation on a profile that is not connected reaches the
        // connection and reports NotConnected; everything below must not.
        assert_eq!(
            run(&id, "probe", "ping", null()).await.unwrap_err(),
            IpcError::NotConnected
        );
        assert_eq!(
            run("not-a-uuid", "probe", "ping", null())
                .await
                .unwrap_err(),
            IpcError::NotFound
        );
        assert!(matches!(
            run(&id, "../x", "ping", null()).await.unwrap_err(),
            IpcError::Validation { field, .. } if field == "moduleId"
        ));
        assert!(matches!(
            run(&id, "probe", "Ping;rm", null()).await.unwrap_err(),
            IpcError::Validation { field, .. } if field == "operationId"
        ));
        let big = serde_json::json!({ "pad": "x".repeat(MAX_ARGS_BYTES) });
        assert!(matches!(
            run(&id, "probe", "ping", big).await.unwrap_err(),
            IpcError::Validation { field, .. } if field == "args"
        ));
        assert_eq!(
            run(&id, "other", "ping", null()).await.unwrap_err(),
            IpcError::NotFound
        );
        assert_eq!(
            run(&id, "probe", "shell", null()).await.unwrap_err(),
            IpcError::NotFound
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn connected_run_passes_context_and_maps_errors() {
        let (svc, reg, calls, id) = setup();
        svc.connect(&id, None, false).await.unwrap();
        let v = run_operation(&svc, &reg, &id, "probe", "ping", null())
            .await
            .unwrap();
        assert!(v["observedAt"].as_str().is_some_and(|s| s.contains('T')));
        assert!(matches!(
            run_operation(&svc, &reg, &id, "probe", "ping", serde_json::json!({"bad": 1}))
                .await
                .unwrap_err(),
            IpcError::Validation { field, .. } if field == "args"
        ));
        assert_eq!(
            run_operation(
                &svc,
                &reg,
                &id,
                "probe",
                "ping",
                serde_json::json!({"big": 1})
            )
            .await
            .unwrap_err(),
            IpcError::OperationFailed {
                message: "result too large".into()
            }
        );
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn list_modules_serializes_camel_case() {
        let (_, reg, _, _) = setup();
        let v = serde_json::to_value(list_modules(&reg)).unwrap();
        assert_eq!(v[0]["id"], "probe");
        assert_eq!(v[0]["operations"][0]["id"], "ping");
        assert_eq!(v[0]["dependencies"], serde_json::json!([]));
    }
}
