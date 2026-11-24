//! Explicit module registration and dependency resolution.
//!
//! [`RegistryBuilder::resolve`] is a pure function over manifests so that the
//! project generator and the runtime registry share one implementation.

use crate::module::{
    Module, ModuleId, ModuleManifest, OperationContext, OperationError, OperationId, OperationInfo,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    #[error("duplicate module id '{0}'")]
    DuplicateId(ModuleId),
    #[error("module '{module}' depends on '{dependency}', which is not registered")]
    MissingDependency {
        module: ModuleId,
        dependency: ModuleId,
    },
    /// Lists the modules that could not be ordered (the cycle and anything
    /// that depends on it), sorted by id.
    #[error("dependency cycle among modules: {}", join_ids(.0))]
    Cycle(Vec<ModuleId>),
    #[error("module '{module}' lists operation '{operation}' more than once")]
    DuplicateOperation { module: ModuleId, operation: String },
    /// A requested module is not among the known manifests.
    #[error("unknown module '{0}'")]
    UnknownModule(ModuleId),
}

fn join_ids(ids: &[ModuleId]) -> String {
    ids.iter()
        .map(ModuleId::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("unknown module '{0}'")]
    UnknownModule(ModuleId),
    #[error("module '{module}' has no operation '{operation}'")]
    UnknownOperation {
        module: ModuleId,
        operation: OperationId,
    },
    #[error(transparent)]
    Operation(#[from] OperationError),
}

#[derive(Default)]
pub struct RegistryBuilder {
    modules: Vec<Box<dyn Module>>,
}

impl RegistryBuilder {
    pub fn register(mut self, module: impl Module + 'static) -> Self {
        self.modules.push(Box::new(module));
        self
    }

    /// Orders manifests with dependencies before dependents and ties by
    /// module id (Kahn's algorithm with a sorted ready set). Rejects duplicate
    /// ids, missing dependencies and cycles.
    pub fn resolve(manifests: &[ModuleManifest]) -> Result<Vec<ModuleId>, RegistryError> {
        let mut ids = BTreeSet::new();
        for m in manifests {
            if !ids.insert(&m.id) {
                return Err(RegistryError::DuplicateId(m.id.clone()));
            }
        }
        for m in manifests {
            if let Some(dep) = m.dependencies.iter().find(|d| !ids.contains(d)) {
                return Err(RegistryError::MissingDependency {
                    module: m.id.clone(),
                    dependency: dep.clone(),
                });
            }
        }

        // Unresolved dependency count and reverse edges. Repeated dependency
        // entries are collapsed.
        let mut pending: BTreeMap<&ModuleId, usize> = BTreeMap::new();
        let mut dependents: BTreeMap<&ModuleId, Vec<&ModuleId>> = BTreeMap::new();
        for m in manifests {
            let deps: BTreeSet<&ModuleId> = m.dependencies.iter().collect();
            pending.insert(&m.id, deps.len());
            for d in deps {
                dependents.entry(d).or_default().push(&m.id);
            }
        }
        let mut ready: BTreeSet<&ModuleId> = pending
            .iter()
            .filter(|(_, n)| **n == 0)
            .map(|(id, _)| *id)
            .collect();

        let mut order = Vec::with_capacity(manifests.len());
        while let Some(id) = ready.pop_first() {
            order.push(id.clone());
            for dep in dependents.get(id).into_iter().flatten() {
                if let Some(n) = pending.get_mut(dep) {
                    *n -= 1;
                    if *n == 0 {
                        ready.insert(dep);
                    }
                }
            }
        }
        if order.len() != manifests.len() {
            let done: BTreeSet<&ModuleId> = order.iter().collect();
            let stuck = ids
                .into_iter()
                .filter(|id| !done.contains(id))
                .cloned()
                .collect();
            return Err(RegistryError::Cycle(stuck));
        }
        Ok(order)
    }

    /// Returns the requested modules plus their required dependencies, in
    /// resolved order, and nothing else. Works on manifests alone.
    pub fn resolve_selection(
        available: &[ModuleManifest],
        requested: &[ModuleId],
    ) -> Result<Vec<ModuleId>, RegistryError> {
        let by_id: BTreeMap<&ModuleId, &ModuleManifest> =
            available.iter().map(|m| (&m.id, m)).collect();
        let mut selected: BTreeMap<&ModuleId, &ModuleManifest> = BTreeMap::new();
        let mut todo: Vec<&ModuleId> = requested.iter().collect();
        while let Some(id) = todo.pop() {
            if selected.contains_key(id) {
                continue;
            }
            let Some(m) = by_id.get(id) else {
                return Err(RegistryError::UnknownModule(id.clone()));
            };
            selected.insert(id, m);
            todo.extend(m.dependencies.iter());
        }
        let subset: Vec<ModuleManifest> = selected.values().map(|m| (*m).clone()).collect();
        Self::resolve(&subset)
    }

    pub fn build(self) -> Result<Registry, RegistryError> {
        let manifests: Vec<ModuleManifest> = self.modules.iter().map(|m| m.manifest()).collect();
        // Check operations before ordering so errors name the module.
        for (module, manifest) in self.modules.iter().zip(&manifests) {
            let mut seen = BTreeSet::new();
            for op in module.operations() {
                if !seen.insert(op.id) {
                    return Err(RegistryError::DuplicateOperation {
                        module: manifest.id.clone(),
                        operation: op.id.to_owned(),
                    });
                }
            }
        }
        let order = RegistryBuilder::resolve(&manifests)?;
        let mut slots: BTreeMap<ModuleId, (ModuleManifest, Box<dyn Module>)> = manifests
            .into_iter()
            .zip(self.modules)
            .map(|(manifest, module)| (manifest.id.clone(), (manifest, module)))
            .collect();
        let entries = order
            .iter()
            .filter_map(|id| slots.remove(id))
            .collect::<Vec<_>>();
        Ok(Registry { entries })
    }
}

/// Modules in resolved order. Built only from explicitly registered modules.
pub struct Registry {
    entries: Vec<(ModuleManifest, Box<dyn Module>)>,
}

impl Registry {
    pub fn builder() -> RegistryBuilder {
        RegistryBuilder::default()
    }

    /// Manifests in resolved order.
    pub fn manifests(&self) -> Vec<ModuleManifest> {
        self.entries.iter().map(|(m, _)| m.clone()).collect()
    }

    /// Manifests with their listed operations, in resolved order.
    pub fn describe(&self) -> Vec<(ModuleManifest, &'static [OperationInfo])> {
        self.entries
            .iter()
            .map(|(m, module)| (m.clone(), module.operations()))
            .collect()
    }

    fn find(&self, id: &ModuleId) -> Option<&dyn Module> {
        self.entries
            .iter()
            .find(|(m, _)| &m.id == id)
            .map(|(_, module)| module.as_ref())
    }

    /// Whether the module exists and lists the operation.
    pub fn has_operation(&self, module: &ModuleId, op: &OperationId) -> bool {
        self.find(module)
            .is_some_and(|m| m.operations().iter().any(|o| o.id == op.as_str()))
    }

    /// Dispatches only to a listed operation of a registered module.
    pub async fn run(
        &self,
        module: &ModuleId,
        op: &OperationId,
        args: serde_json::Value,
        ctx: &OperationContext<'_>,
    ) -> Result<serde_json::Value, RunError> {
        let m = self
            .find(module)
            .ok_or_else(|| RunError::UnknownModule(module.clone()))?;
        if !m.operations().iter().any(|o| o.id == op.as_str()) {
            return Err(RunError::UnknownOperation {
                module: module.clone(),
                operation: op.clone(),
            });
        }
        Ok(m.run(op.as_str(), args, ctx).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::{CancelToken, ExecError, ExecResult, Executor};
    use crate::{CommandSpec, OperationInfo};
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static ONE_OP: &[OperationInfo] = &[OperationInfo {
        id: "ping",
        description: "test",
    }];
    static TWO_SAME_OPS: &[OperationInfo] = &[
        OperationInfo {
            id: "ping",
            description: "a",
        },
        OperationInfo {
            id: "ping",
            description: "b",
        },
    ];

    struct Fake {
        id: &'static str,
        deps: &'static [&'static str],
        ops: &'static [OperationInfo],
        calls: Arc<AtomicUsize>,
    }

    fn fake(id: &'static str, deps: &'static [&'static str]) -> Fake {
        Fake {
            id,
            deps,
            ops: ONE_OP,
            calls: Arc::default(),
        }
    }

    #[async_trait]
    impl Module for Fake {
        fn manifest(&self) -> ModuleManifest {
            ModuleManifest {
                id: ModuleId::parse(self.id).unwrap(),
                name: self.id.to_owned(),
                version: "0.0.0".to_owned(),
                description: String::new(),
                dependencies: self
                    .deps
                    .iter()
                    .map(|d| ModuleId::parse(d).unwrap())
                    .collect(),
            }
        }
        fn operations(&self) -> &'static [OperationInfo] {
            self.ops
        }
        async fn run(
            &self,
            _operation: &str,
            _args: serde_json::Value,
            _ctx: &OperationContext<'_>,
        ) -> Result<serde_json::Value, OperationError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!("ok"))
        }
    }

    struct NoExec;

    #[async_trait]
    impl Executor for NoExec {
        async fn run(&self, _: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
            Err(ExecError::Cancelled)
        }
    }

    fn mid(s: &str) -> ModuleId {
        ModuleId::parse(s).unwrap()
    }

    fn manifest(id: &str, deps: &[&str]) -> ModuleManifest {
        ModuleManifest {
            id: mid(id),
            name: id.to_owned(),
            version: "0.0.0".to_owned(),
            description: String::new(),
            dependencies: deps.iter().map(|d| mid(d)).collect(),
        }
    }

    fn ids(v: &[ModuleId]) -> Vec<&str> {
        v.iter().map(ModuleId::as_str).collect()
    }

    #[test]
    fn duplicate_id_is_rejected() {
        let err = Registry::builder()
            .register(fake("a", &[]))
            .register(fake("a", &[]))
            .build()
            .err()
            .unwrap();
        assert_eq!(err, RegistryError::DuplicateId(mid("a")));
    }

    #[test]
    fn missing_dependency_is_rejected() {
        let err = Registry::builder()
            .register(fake("a", &["b"]))
            .build()
            .err()
            .unwrap();
        assert_eq!(
            err,
            RegistryError::MissingDependency {
                module: mid("a"),
                dependency: mid("b")
            }
        );
    }

    #[test]
    fn cycle_lists_unresolved_ids() {
        let err = RegistryBuilder::resolve(&[
            manifest("ok", &[]),
            manifest("a", &["b"]),
            manifest("b", &["a"]),
        ])
        .unwrap_err();
        assert_eq!(err, RegistryError::Cycle(vec![mid("a"), mid("b")]));
        assert_eq!(err.to_string(), "dependency cycle among modules: a, b");
    }

    #[test]
    fn duplicate_operation_is_rejected() {
        let mut m = fake("a", &[]);
        m.ops = TWO_SAME_OPS;
        let err = Registry::builder().register(m).build().err().unwrap();
        assert_eq!(
            err,
            RegistryError::DuplicateOperation {
                module: mid("a"),
                operation: "ping".to_owned()
            }
        );
    }

    #[test]
    fn order_is_dependencies_first_then_by_id() {
        let registry = Registry::builder()
            .register(fake("zeta", &["alpha"]))
            .register(fake("beta", &[]))
            .register(fake("alpha", &[]))
            .register(fake("app", &["zeta", "beta"]))
            .build()
            .unwrap();
        let order: Vec<ModuleId> = registry.manifests().into_iter().map(|m| m.id).collect();
        assert_eq!(ids(&order), ["alpha", "beta", "zeta", "app"]);
    }

    #[test]
    fn selection_includes_only_requested_and_required_modules() {
        let all = [
            manifest("base", &[]),
            manifest("web", &["base"]),
            manifest("other", &[]),
            manifest("unrelated", &["other"]),
        ];
        let order = RegistryBuilder::resolve_selection(&all, &[mid("web")]).unwrap();
        assert_eq!(ids(&order), ["base", "web"]);
        assert!(
            RegistryBuilder::resolve_selection(&all, &[])
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            RegistryBuilder::resolve_selection(&all, &[mid("nope")]).unwrap_err(),
            RegistryError::UnknownModule(mid("nope"))
        );
    }

    #[tokio::test]
    async fn unknown_module_or_operation_is_not_dispatched() {
        let m = fake("a", &[]);
        let calls = m.calls.clone();
        let registry = Registry::builder().register(m).build().unwrap();
        let cancel = CancelToken::new();
        let ctx = OperationContext {
            exec: &NoExec,
            cancel: &cancel,
            observed_at: String::new(),
        };
        let null = serde_json::Value::Null;
        let bad_op = OperationId::parse("nope").unwrap();
        let err = registry
            .run(&mid("a"), &bad_op, null.clone(), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, RunError::UnknownOperation { .. }));
        let ping = OperationId::parse("ping").unwrap();
        let err = registry
            .run(&mid("zzz"), &ping, null.clone(), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, RunError::UnknownModule(_)));
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        assert!(registry.has_operation(&mid("a"), &ping));
        registry.run(&mid("a"), &ping, null, &ctx).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
