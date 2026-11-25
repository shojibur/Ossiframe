//! Keeps the local application records in step with operations.
//!
//! The engine reports every state change through `OperationEvents`. When an
//! application operation reaches a terminal state this sink settles the app
//! and release rows from the stored plan and step records alone, so it works
//! the same for an operation that ended normally, after a reconcile, or after
//! "mark resolved". Errors are ignored: the UI reads the operation itself,
//! and a missed update is visible there and is corrected by the next
//! operation.
use super::dto::release_row_id;
use super::plans::{Kind, plan_kind};
use ossiframe_core::provision::{OperationState, Plan};
use ossiframe_desktop::provisioning::engine::OperationEvents;
use ossiframe_desktop::store::{
    APP_STATE_ACTIVE, APP_STATE_AWAITING_DEPLOY, APP_STATE_FAILED, APP_STATE_REMOVED, AppRecord,
    StepRecord, Store,
};
use ossiframe_mod_app_deploy::release::{STEP_COMMIT, STEP_SWITCH, parse_commit_sha};
use ossiframe_mod_app_deploy::verify::release_of_plan;
use std::sync::Arc;

/// Release row statuses.
pub const RELEASE_PENDING: &str = "pending";
pub const RELEASE_DEPLOYED: &str = "deployed";
pub const RELEASE_FAILED: &str = "failed";
pub const RELEASE_ROLLED_BACK_FROM: &str = "rolledBackFrom";
pub const RELEASE_ROLLED_BACK_TO: &str = "rolledBackTo";

/// Wraps the UI event sink and settles app records on terminal states.
pub struct AppEvents {
    inner: Arc<dyn OperationEvents>,
    store: Arc<Store>,
}

impl AppEvents {
    pub fn new(inner: Arc<dyn OperationEvents>, store: Arc<Store>) -> Self {
        Self { inner, store }
    }
}

impl OperationEvents for AppEvents {
    fn operation_updated(&self, server_id: &str, operation_id: &str, state: OperationState) {
        // Settle first, so the UI reads consistent records when it is told.
        if state.is_terminal() {
            settle(&self.store, operation_id);
        }
        self.inner.operation_updated(server_id, operation_id, state);
    }
}

/// First line of a step's stored output, parsed as a commit.
fn commit_of(steps: &[StepRecord]) -> Option<String> {
    steps
        .iter()
        .find(|s| s.step_id == STEP_COMMIT)
        .and_then(|s| s.output.as_deref())
        .and_then(|o| parse_commit_sha(o.lines().next().unwrap_or("")))
}

fn status_of<'a>(steps: &'a [StepRecord], id: &str) -> Option<&'a str> {
    steps
        .iter()
        .find(|s| s.step_id == id)
        .map(|s| s.status.as_str())
}

/// Applies the end of operation `operation_id` to the app and release rows.
/// Does nothing for a non-terminal or non-application operation.
pub fn settle(store: &Store, operation_id: &str) {
    let Ok(op) = store.get_operation(operation_id) else {
        return;
    };
    if op.kind != "app-deploy" || !op.state.is_terminal() {
        return;
    }
    let Ok(plan) = serde_json::from_str::<Plan>(&op.plan_json) else {
        return;
    };
    let Some(app_id) = plan.target.app_id.clone() else {
        return;
    };
    let Ok(app) = store.get_app(&app_id) else {
        return;
    };
    if app.state == APP_STATE_REMOVED {
        return;
    }
    let Some(kind) = plan_kind(&plan) else {
        return;
    };
    let steps = store.get_steps(operation_id).unwrap_or_default();
    let ok = op.state == OperationState::Succeeded;
    let cx = Cx {
        store,
        app: &app,
        plan: &plan,
        steps: &steps,
        op_id: operation_id,
        ok,
        cancelled: op.state == OperationState::Cancelled,
    };
    match kind {
        Kind::Create { deploy: false } => cx.state(if ok {
            APP_STATE_AWAITING_DEPLOY
        } else {
            APP_STATE_FAILED
        }),
        Kind::Create { deploy: true } => cx.release(true),
        Kind::Deploy => cx.release(false),
        Kind::Rollback => cx.rollback(),
        Kind::Remove => {
            if ok {
                cx.state(APP_STATE_REMOVED);
            } else if !cx.cancelled {
                cx.state(APP_STATE_FAILED);
            }
        }
        Kind::EnvSet => {}
    }
}

struct Cx<'a> {
    store: &'a Store,
    app: &'a AppRecord,
    plan: &'a Plan,
    steps: &'a [StepRecord],
    op_id: &'a str,
    ok: bool,
    cancelled: bool,
}

impl Cx<'_> {
    fn state(&self, s: &str) {
        let _ = self.store.set_app_state(&self.app.id, s);
    }

    /// The state of an app with no operation running: serving a release, or
    /// waiting for its first one.
    fn idle(&self) -> &'static str {
        if self.app.current_release_id.is_some() {
            APP_STATE_ACTIVE
        } else {
            APP_STATE_AWAITING_DEPLOY
        }
    }

    /// A create-with-deploy or a deploy.
    ///
    /// If the switch ran, `current` is the new release even when the checks
    /// then failed (no automatic rollback): the release is recorded as
    /// deployed and the app as failed so the user is offered a rollback.
    fn release(&self, create: bool) {
        let switched = status_of(self.steps, STEP_SWITCH) == Some("succeeded");
        if let Ok(release) = release_of_plan(self.plan) {
            let row = release_row_id(&self.app.id, &release);
            let commit = commit_of(self.steps);
            let live = self.ok || switched;
            let _ = self.store.update_release(
                &self.app.id,
                &row,
                if live {
                    RELEASE_DEPLOYED
                } else {
                    RELEASE_FAILED
                },
                commit.as_deref(),
                Some(self.op_id),
            );
            if live {
                let _ = self.store.set_current_release(&self.app.id, Some(&row));
            }
        }
        let state = if self.ok {
            APP_STATE_ACTIVE
        } else if switched || create {
            APP_STATE_FAILED
        } else {
            self.idle()
        };
        self.state(state);
    }

    fn rollback(&self) {
        let switched = status_of(self.steps, STEP_SWITCH) == Some("succeeded");
        if self.ok || switched {
            if let Ok(release) = release_of_plan(self.plan) {
                let row = release_row_id(&self.app.id, &release);
                if let Some(from) = self.app.current_release_id.as_deref()
                    && from != row
                {
                    let _ = self.store.update_release(
                        &self.app.id,
                        from,
                        RELEASE_ROLLED_BACK_FROM,
                        None,
                        None,
                    );
                }
                let _ = self.store.update_release(
                    &self.app.id,
                    &row,
                    RELEASE_ROLLED_BACK_TO,
                    None,
                    Some(self.op_id),
                );
                let _ = self.store.set_current_release(&self.app.id, Some(&row));
            }
            self.state(if self.ok {
                APP_STATE_ACTIVE
            } else {
                APP_STATE_FAILED
            });
        } else {
            self.state(self.idle());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::dto::CreateParams;
    use crate::apps::plans::{assemble, build_create, build_redeploy};
    use ossiframe_core::{BranchName, Hostname, Port, ServerName, SshUser};
    use ossiframe_desktop::store::{
        APP_STATE_DEPLOYING, AuthConfig, NewOperation, OperationIdentity, ReleaseRecord,
        ServerRecord, timestamp,
    };
    use ossiframe_mod_app_deploy::create::DbPassword;
    use ossiframe_mod_app_deploy::git_access::HostKeys;
    use ossiframe_mod_app_deploy::release::{ReleaseId, STEP_CLONE};
    use ossiframe_mod_app_deploy::remove::{RemoveInput, remove_steps};

    const SERVER: &str = "6f1d3f0e-0000-4000-8000-000000000001";
    const APP: &str = "11111111-1111-4111-8111-111111111111";
    const OLD: &str = "20250101T000000Z";
    const NEW: &str = "20250202T000000Z";
    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    /// A store with one active app that already serves release `OLD`.
    fn setup() -> (Store, AppRecord) {
        let store = Store::open_in_memory().unwrap();
        store
            .insert(&ServerRecord {
                id: SERVER.into(),
                name: ServerName::parse("web").unwrap(),
                host: Hostname::parse("example.com").unwrap(),
                port: Port::new(22).unwrap(),
                username: SshUser::parse("deploy").unwrap(),
                auth: AuthConfig::Password,
                secret_ref: None,
                created_at: timestamp(),
                updated_at: timestamp(),
            })
            .unwrap();
        let i = CreateParams {
            server_id: SERVER.into(),
            app_type: "static".into(),
            slug: "blog".into(),
            domain: "blog.example.com".into(),
            repo_url: "https://github.com/o/r".into(),
            repo_branch: "main".into(),
            entry_file: None,
            with_database: false,
            disable_default_site: false,
            trusted_git_host_fingerprints: None,
        }
        .parse()
        .unwrap();
        let mut app = build_create(
            &i,
            SERVER,
            APP,
            &HostKeys::None,
            &DbPassword::parse("Zq8Xk2Lm9Pv4Rt7Yb1Nc6Hd3Jf5Wg0Sa").unwrap(),
            ReleaseId::parse(OLD).unwrap(),
        )
        .unwrap()
        .app;
        app.state = APP_STATE_ACTIVE.to_owned();
        app.current_release_id = Some(format!("{APP}:{OLD}"));
        store.insert_app(&app).unwrap();
        for (r, status) in [(OLD, RELEASE_DEPLOYED), (NEW, RELEASE_PENDING)] {
            store
                .insert_release(&ReleaseRecord {
                    id: format!("{APP}:{r}"),
                    app_id: APP.into(),
                    commit_sha: None,
                    branch: "main".into(),
                    status: status.into(),
                    operation_id: None,
                    created_at: timestamp(),
                })
                .unwrap();
        }
        store
            .set_current_release(APP, Some(&format!("{APP}:{OLD}")))
            .unwrap();
        (store, app)
    }

    /// Records an operation for `plan` ending in `state`, with the given
    /// step statuses (every other step stays pending).
    fn finish(
        store: &Store,
        app: &AppRecord,
        plan: &ossiframe_core::provision::Plan,
        state: OperationState,
        done: &[&str],
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        store
            .insert_operation(&NewOperation {
                id: id.clone(),
                identity: OperationIdentity {
                    server_id: SERVER.into(),
                    name: "web".into(),
                    host: "example.com".into(),
                    port: 22,
                    user: "deploy".into(),
                    host_key_sha256: "k".into(),
                },
                kind: "app-deploy".into(),
                plan_digest: "0".repeat(64),
                plan_json: serde_json::to_string(plan).unwrap(),
            })
            .unwrap();
        for (i, s) in plan.steps.iter().enumerate() {
            let status = if done.contains(&s.id.as_str()) {
                "succeeded"
            } else {
                "pending"
            };
            store
                .upsert_step(
                    &id,
                    &StepRecord {
                        idx: i as u32,
                        step_id: s.id.clone(),
                        title: s.title.clone(),
                        unit_name: None,
                        status: status.into(),
                        started_at: None,
                        finished_at: None,
                        exit_code: None,
                        output: None,
                    },
                )
                .unwrap();
            if s.id == STEP_COMMIT && status == "succeeded" {
                store
                    .replace_step_output(&id, i as u32, format!("{SHA}\n").as_bytes(), &[])
                    .unwrap();
            }
        }
        store.set_app_state(&app.id, APP_STATE_DEPLOYING).unwrap();
        store.update_operation_status(&id, state, None).unwrap();
        id
    }

    fn deploy_plan(app: &AppRecord) -> ossiframe_core::provision::Plan {
        let pieces = build_redeploy(
            app,
            &BranchName::parse("main").unwrap(),
            &ReleaseId::parse(NEW).unwrap(),
            false,
            None,
        )
        .unwrap();
        assemble(&app.id, pieces, Default::default(), true)
    }

    fn release(store: &Store, id: &str) -> ReleaseRecord {
        store
            .list_releases(APP)
            .unwrap()
            .into_iter()
            .find(|r| r.id == format!("{APP}:{id}"))
            .unwrap()
    }

    #[test]
    fn a_successful_deploy_makes_the_new_release_current() {
        let (store, app) = setup();
        let plan = deploy_plan(&app);
        let op = finish(
            &store,
            &app,
            &plan,
            OperationState::Succeeded,
            &[STEP_CLONE, STEP_COMMIT, STEP_SWITCH],
        );
        settle(&store, &op);
        let a = store.get_app(APP).unwrap();
        assert_eq!(a.state, APP_STATE_ACTIVE);
        assert_eq!(
            a.current_release_id.as_deref(),
            Some(&*format!("{APP}:{NEW}"))
        );
        let r = release(&store, NEW);
        assert_eq!(r.status, RELEASE_DEPLOYED);
        assert_eq!(r.commit_sha.as_deref(), Some(SHA));
        assert_eq!(r.operation_id.as_deref(), Some(op.as_str()));
    }

    #[test]
    fn a_failure_before_the_switch_keeps_current_and_marks_the_release_failed() {
        let (store, app) = setup();
        let plan = deploy_plan(&app);
        let op = finish(&store, &app, &plan, OperationState::Failed, &[STEP_CLONE]);
        settle(&store, &op);
        let a = store.get_app(APP).unwrap();
        assert_eq!(a.state, APP_STATE_ACTIVE);
        assert_eq!(
            a.current_release_id.as_deref(),
            Some(&*format!("{APP}:{OLD}"))
        );
        assert_eq!(release(&store, NEW).status, RELEASE_FAILED);
    }

    #[test]
    fn a_failed_check_after_the_switch_leaves_current_on_the_new_release() {
        let (store, app) = setup();
        let plan = deploy_plan(&app);
        let op = finish(
            &store,
            &app,
            &plan,
            OperationState::Failed,
            &[STEP_CLONE, STEP_COMMIT, STEP_SWITCH],
        );
        settle(&store, &op);
        let a = store.get_app(APP).unwrap();
        // No automatic rollback: the app needs attention and offers one.
        assert_eq!(a.state, APP_STATE_FAILED);
        assert_eq!(
            a.current_release_id.as_deref(),
            Some(&*format!("{APP}:{NEW}"))
        );
        assert_eq!(release(&store, NEW).status, RELEASE_DEPLOYED);
    }

    #[test]
    fn a_successful_removal_marks_the_app_removed_and_a_failed_one_does_not() {
        for (state, want) in [
            (OperationState::Succeeded, APP_STATE_REMOVED),
            (OperationState::Failed, APP_STATE_FAILED),
            (OperationState::Cancelled, APP_STATE_DEPLOYING),
        ] {
            let (store, app) = setup();
            let steps = remove_steps(&RemoveInput {
                app_type: ossiframe_mod_app_deploy::app_type::AppType::Static,
                slug: ossiframe_mod_app_deploy::identity::app_slug("blog").unwrap(),
                drop_database: false,
                delete_files: false,
            })
            .unwrap();
            let plan = assemble(&app.id, steps.into(), Default::default(), false);
            let op = finish(&store, &app, &plan, state, &[]);
            settle(&store, &op);
            assert_eq!(store.get_app(APP).unwrap().state, want, "{state:?}");
        }
    }

    #[test]
    fn a_non_terminal_or_foreign_operation_changes_nothing() {
        let (store, app) = setup();
        let plan = deploy_plan(&app);
        let op = finish(&store, &app, &plan, OperationState::Running, &[]);
        settle(&store, &op);
        assert_eq!(store.get_app(APP).unwrap().state, APP_STATE_DEPLOYING);
        settle(&store, "no-such-operation");
    }
}
