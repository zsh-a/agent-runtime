use super::*;

impl RuntimeServer {
    pub(crate) async fn new(options: RuntimeServerOptions) -> Result<Self> {
        let RuntimeServerOptions {
            sources,
            store_path,
            store_backend,
            mut tool_overrides,
            hooks,
            context_policy,
            default_agent,
            chat,
        } = options;
        info!(
            registry = %sources.registry,
            catalog = sources.catalog.as_ref().map(|path| path.as_str()).unwrap_or("none"),
            store = %store_path,
            store_backend = ?store_backend,
            "initializing runtime server",
        );
        let composition = Arc::new(
            compose_runtime_sources(RuntimeSourceOptions {
                sources,
                tool_overrides: tool_overrides.clone(),
            })
            .await?,
        );
        tool_overrides.extend_tool_specs(composition.tool_specs.clone());
        let catalog = Arc::new(composition.catalog_view.clone());
        info!(
            agent_count = catalog.agents.len(),
            tool_count = catalog.tools.len(),
            proposal_kind_count = catalog.proposal_kinds.len(),
            active_domains = ?catalog.active_domains,
            "runtime server catalog loaded",
        );
        let stores = RuntimeStores::open(store_backend, store_path).await?;
        let store_path = stores.artifact_store_path.clone();
        let services = Arc::new(CliServices::with_stores(
            tool_overrides,
            stores.state_store.clone(),
            stores.proposal_store.clone(),
        ));
        let runner = Arc::new(
            AgentRunner::new_with_factory(
                composition.registry.clone(),
                stores.run_store.clone(),
                services.clone(),
            )
            .with_lock_store(stores.lock_store.clone())
            .with_hooks(hooks.clone()),
        );
        Ok(Self {
            catalog,
            composition,
            runner,
            services,
            chat,
            context_policy,
            default_agent,
            hooks,
            run_store: stores.run_store,
            event_store: stores.event_store,
            trace_store: stores.trace_store,
            proposal_store: stores.proposal_store,
            session_store: stores.session_store,
            store_path,
            active_runs: Arc::new(Mutex::new(HashMap::new())),
        })
    }
}
