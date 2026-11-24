//! Example module `ports`: read-only list of listening TCP sockets.
//!
//! Uses only the public `ossiframe-core` API. The operation `listening-tcp`
//! runs the fixed command `ss -Htln`, parses the bounded output in Rust and
//! applies the optional `family` filter to the parsed results only.
//! Modules are trusted code; the manifest is not a sandbox.
pub mod parse;

use async_trait::async_trait;
use ossiframe_core::{
    CommandSpec, Module, ModuleId, ModuleManifest, OperationContext, OperationError, OperationInfo,
};
use parse::{Family, Listening};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const OPERATIONS: &[OperationInfo] = &[OperationInfo {
    id: "listening-tcp",
    description: "Listening TCP sockets (local address and port)",
}];

const TIMEOUT: Duration = Duration::from_secs(10);

/// Which address family to report. Applied to parsed results, never to argv.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FamilyFilter {
    Ipv4,
    Ipv6,
    All,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    family: Option<FamilyFilter>,
}

/// `null` means no filter. Anything else must match the documented shape.
pub fn parse_args(args: serde_json::Value) -> Result<FamilyFilter, OperationError> {
    if args.is_null() {
        return Ok(FamilyFilter::All);
    }
    let parsed: Args = serde_json::from_value(args).map_err(|_| {
        OperationError::InvalidArguments(
            "expected null or {\"family\": \"ipv4\" | \"ipv6\" | \"all\"}".to_owned(),
        )
    })?;
    Ok(parsed.family.unwrap_or(FamilyFilter::All))
}

impl FamilyFilter {
    fn allows(self, family: Family) -> bool {
        matches!(
            (self, family),
            (FamilyFilter::All, _)
                | (_, Family::Any)
                | (FamilyFilter::Ipv4, Family::V4)
                | (FamilyFilter::Ipv6, Family::V6)
        )
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Socket {
    local_address: String,
    port: u16,
    process: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PortsResult {
    observed_at: String,
    sockets: Vec<Socket>,
    skipped: usize,
    truncated: bool,
}

pub struct Ports;

#[async_trait]
impl Module for Ports {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: ModuleId::parse("ports").expect("valid module id"),
            name: "Ports (example)".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "Read-only list of listening TCP sockets".to_owned(),
            dependencies: Vec::new(),
        }
    }

    fn operations(&self) -> &'static [OperationInfo] {
        OPERATIONS
    }

    async fn run(
        &self,
        operation: &str,
        args: serde_json::Value,
        ctx: &OperationContext<'_>,
    ) -> Result<serde_json::Value, OperationError> {
        if operation != "listening-tcp" {
            return Err(OperationError::Failed("unknown operation".to_owned()));
        }
        let filter = parse_args(args)?;
        // Fixed program and argv. The filter never reaches the command.
        let spec = CommandSpec::new("ss", &["-Htln"], TIMEOUT)
            .map_err(|_| OperationError::Failed("invalid command".to_owned()))?;
        let out = ctx.exec.run(&spec, ctx.cancel).await?;
        if !out.exit.success() {
            return Err(OperationError::Failed("ss did not succeed".to_owned()));
        }
        let parsed = parse::parse_ss(&out.stdout);
        let sockets = parsed
            .sockets
            .into_iter()
            .filter(|s| filter.allows(s.family))
            .map(|Listening { address, port, .. }| Socket {
                local_address: address,
                port,
                process: None,
            })
            .collect();
        let result = PortsResult {
            observed_at: ctx.observed_at.clone(),
            sockets,
            skipped: parsed.skipped,
            truncated: parsed.truncated || out.truncated,
        };
        serde_json::to_value(result)
            .map_err(|_| OperationError::Failed("could not encode result".to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ossiframe_core::{CancelToken, ExecError, ExecResult, Executor, ExitStatus};
    use serde_json::json;
    use std::sync::Mutex;

    struct Fake {
        stdout: &'static str,
        seen: Mutex<Vec<CommandSpec>>,
    }

    #[async_trait]
    impl Executor for Fake {
        async fn run(&self, spec: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
            self.seen.lock().unwrap().push(spec.clone());
            Ok(ExecResult {
                exit: ExitStatus::Code(0),
                stdout: self.stdout.to_owned(),
                stderr: String::new(),
                truncated: false,
            })
        }
    }

    #[test]
    fn family_args_are_validated() {
        assert_eq!(parse_args(json!(null)).unwrap(), FamilyFilter::All);
        assert_eq!(parse_args(json!({})).unwrap(), FamilyFilter::All);
        assert_eq!(
            parse_args(json!({"family": "ipv6"})).unwrap(),
            FamilyFilter::Ipv6
        );
        for bad in [
            json!({"family": "ipx"}),
            json!({"family": "IPV4"}),
            json!({"family": 4}),
            json!({"family": "all", "extra": 1}),
            json!({"x": "ipv4"}),
            json!("ipv4"),
            json!([]),
        ] {
            assert!(
                matches!(parse_args(bad), Err(OperationError::InvalidArguments(_))),
                "should reject"
            );
        }
    }

    #[tokio::test]
    async fn runs_fixed_argv_and_filters_parsed_results() {
        let fake = Fake {
            stdout: "LISTEN 0 128 0.0.0.0:22 0.0.0.0:*\nLISTEN 0 128 [::]:22 [::]:*\n",
            seen: Mutex::new(Vec::new()),
        };
        let cancel = CancelToken::new();
        let ctx = OperationContext {
            exec: &fake,
            cancel: &cancel,
            observed_at: "t".to_owned(),
        };
        let v = Ports
            .run("listening-tcp", json!({"family": "ipv6"}), &ctx)
            .await
            .unwrap();
        assert_eq!(v["sockets"].as_array().unwrap().len(), 1);
        assert_eq!(v["sockets"][0]["localAddress"], "[::]");
        assert_eq!(v["sockets"][0]["port"], 22);
        assert_eq!(v["observedAt"], "t");
        let seen = fake.seen.lock().unwrap();
        assert_eq!(seen[0].program(), "ss");
        assert_eq!(seen[0].args(), ["-Htln"]);
    }

    #[tokio::test]
    async fn invalid_args_never_reach_the_executor() {
        struct Unreachable;
        #[async_trait]
        impl Executor for Unreachable {
            async fn run(&self, _: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
                panic!("executor must not be reached");
            }
        }
        let cancel = CancelToken::new();
        let ctx = OperationContext {
            exec: &Unreachable,
            cancel: &cancel,
            observed_at: String::new(),
        };
        let err = Ports
            .run("listening-tcp", json!({"family": "; rm"}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, OperationError::InvalidArguments(_)));
    }
}
