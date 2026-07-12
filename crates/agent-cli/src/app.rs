use super::*;

pub(crate) async fn run() -> Result<()> {
    let cli = Cli::parse();
    let context =
        AppContext::new(load_agent_config(cli.config.clone(), cli.profile.as_deref()).await?);
    init_logging(log_mode_for_command(&cli.command, &context));

    match cli.command {
        Command::List { registry, catalog } => {
            let sources = context.runtime_sources(registry, catalog);
            let composition = compose_runtime_sources(RuntimeSourceOptions {
                sources,
                tool_overrides: ToolOverrides::default(),
            })
            .await?;
            let specs = composition.agent_specs;
            println!(
                "{}",
                serde_json::to_string_pretty(&specs).into_diagnostic()?
            );
        }
        Command::Run {
            agent_id,
            registry,
            catalog,
            tools,
            input,
            trace_out,
            session,
            thread,
            scope,
            store,
            timeout_seconds,
            max_retries,
            retry_backoff_ms,
        } => {
            let sources = context.runtime_sources(registry, catalog);
            let store = context.store(store);
            let execution = context.execution(timeout_seconds, max_retries, retry_backoff_ms);
            let hooks = context.hooks()?;
            run_agent_once(RunCliOptions {
                agent_id,
                sources,
                tool_overrides: context.tools(tools).load().await?,
                input,
                trace_out,
                session,
                thread,
                scope,
                store,
                store_backend: context.store_backend(),
                timeout_seconds: execution.timeout_seconds,
                max_retries: execution.max_retries,
                retry_backoff_ms: execution.retry_backoff_ms,
                hooks,
            })
            .await?;
        }
        Command::Tick {
            registry,
            catalog,
            store,
        } => {
            let sources = context.runtime_sources(registry, catalog);
            let store = context.store(store);
            let hooks = context.hooks()?;
            tick_agents(TickCliOptions {
                sources,
                store,
                store_backend: context.store_backend(),
                hooks,
            })
            .await?;
        }
        Command::Replay {
            trace_file,
            mode,
            registry,
            catalog,
            tools,
            store,
            trace_out,
            timeout_seconds,
            max_retries,
            retry_backoff_ms,
        } => {
            let sources = context.runtime_sources(registry, catalog);
            let store = context.store(store);
            let execution = context.execution(timeout_seconds, max_retries, retry_backoff_ms);
            match mode {
                ReplayMode::Live | ReplayMode::Deterministic => {
                    replay_trace(ReplayTraceOptions {
                        trace_file,
                        mode,
                        sources,
                        tools: context.tools(tools),
                        store,
                        store_backend: context.store_backend(),
                        trace_out,
                        timeout_seconds: execution.timeout_seconds,
                        max_retries: execution.max_retries,
                        retry_backoff_ms: execution.retry_backoff_ms,
                        hooks: match mode {
                            ReplayMode::Live => context.hooks()?,
                            ReplayMode::Deterministic => HookManager::default(),
                            ReplayMode::View => unreachable!("view replay does not execute"),
                        },
                    })
                    .await?;
                }
                ReplayMode::View => {
                    let trace = read_json(trace_file).await?;
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&trace).into_diagnostic()?
                    );
                }
            }
        }
        Command::Inspect { run_id, store } => {
            let store = context.store(store);
            let stores = RuntimeStores::open(context.store_backend(), store).await?;
            let record = stores
                .run_store
                .get_run(&RunId(run_id.clone()))
                .await
                .into_diagnostic()?
                .ok_or_else(|| miette!("run '{run_id}' was not found"))?;
            print_json(&record)?;
        }
        Command::Validate { schema, instance } => {
            let report = validate_json(schema, instance).await?;
            print_json(&report)?;
            if !report.valid {
                return Err(miette!("JSON instance failed schema validation"));
            }
        }
        Command::DebugBundle { command } => match command {
            DebugBundleCommand::Export {
                run_id,
                store,
                out,
                catalog,
                trace,
                timeout_seconds,
                materialize_artifacts,
                artifact_resolver,
            } => {
                export_debug_bundle(DebugBundleOptions {
                    run_id,
                    store_path: context.store(store),
                    store_backend: context.store_backend(),
                    out,
                    catalog_path: catalog,
                    trace_path: trace,
                    timeout_seconds,
                    materialize_artifacts,
                    artifact_resolver_path: artifact_resolver,
                })
                .await?;
            }
        },
        Command::Metrics { command } => match command {
            MetricsCommand::Summary { store } => {
                let stores =
                    RuntimeStores::open(context.store_backend(), context.store(store)).await?;
                let summary = build_metrics_summary(
                    &stores.artifact_store_path,
                    stores.run_store.as_ref(),
                    stores.trace_store.as_ref(),
                    stores.proposal_store.as_ref(),
                )
                .await?;
                print_json(&summary)?;
            }
        },
        Command::Trace { command } => match command {
            TraceCommand::ExportOtel {
                trace_file,
                out,
                endpoint,
                header,
                timeout_seconds,
            } => {
                export_otel_trace_file(ExportOtelTraceOptions {
                    trace_file,
                    out,
                    endpoint,
                    header,
                    timeout_seconds,
                })
                .await?;
            }
        },
        Command::Workflow { command } => match command {
            WorkflowCommand::Run {
                input,
                registry,
                catalog,
                store,
                tools,
                timeout_seconds,
                max_retries,
                retry_backoff_ms,
            } => {
                let execution = context.execution(timeout_seconds, max_retries, retry_backoff_ms);
                let sources = context.runtime_sources(registry, catalog);
                let result = run_workflow_request(WorkflowRunCliOptions {
                    input,
                    sources,
                    store: context.store(store),
                    store_backend: context.store_backend(),
                    tools: context.tools(tools),
                    timeout_seconds: execution.timeout_seconds,
                    max_retries: execution.max_retries,
                    retry_backoff_ms: execution.retry_backoff_ms,
                    hooks: context.hooks()?,
                })
                .await?;
                print_json(&result)?;
            }
        },
        Command::Tool { command } => {
            run_tool_command(command).await?;
        }
        Command::Proposal { command } => {
            run_proposal_command(
                command,
                context.hooks()?,
                context.store_backend(),
                context.configured_store(),
            )
            .await?;
        }
        Command::Session { command } => {
            run_session_command(command, context.store_backend(), context.configured_store())
                .await?;
        }
        Command::Llm { command } => match command {
            LlmCommand::Complete {
                prompt,
                provider,
                model,
                mock_response,
                api_base_url,
                api_key_env,
                temperature,
                max_output_tokens,
                anthropic_version,
            } => {
                run_llm_complete(LlmCompleteOptions {
                    prompt,
                    provider,
                    model,
                    mock_response,
                    api_base_url,
                    api_key_env,
                    temperature,
                    max_output_tokens,
                    anthropic_version,
                })
                .await?;
            }
        },
        Command::Catalog { command } => {
            run_catalog_command(command).await?;
        }
        Command::Compat { command } => {
            run_compat_command(command, context.store_backend()).await?;
        }
        Command::Config { command } => match command {
            ConfigCommand::Show => {
                print_json(context.config())?;
            }
        },
        Command::Recover {
            store,
            timeout_seconds,
        } => {
            let store = context.store(store);
            let execution = context.execution(timeout_seconds, 0, 0);
            let stores = RuntimeStores::open(context.store_backend(), store).await?;
            let report = recover_stale_runs(
                stores.run_store.as_ref(),
                &ExecutionPolicy {
                    timeout: Duration::from_secs(execution.timeout_seconds),
                    max_retries: 0,
                    retry_backoff: Duration::ZERO,
                    max_concurrent_runs: 1,
                },
            )
            .await
            .into_diagnostic()?;
            print_json(&report)?;
        }
        Command::Cmd { command } => match command {
            CmdCommand::Create {
                from_run,
                store,
                out,
                description,
                catalog,
                registry,
            } => {
                let sources = context.command_runtime_sources(registry, catalog);
                let report = create_command_from_run(
                    from_run,
                    context.store(store),
                    context.store_backend(),
                    out,
                    description,
                    sources,
                )
                .await?;
                print_json(&report)?;
            }
            CmdCommand::Run {
                command_file,
                catalog,
                registry,
                store,
                tools,
                trace_out,
                timeout_seconds,
                max_retries,
                retry_backoff_ms,
            } => {
                let report = run_command_template(CommandRunOptions {
                    command_file,
                    configured_sources: context.configured_runtime_sources(),
                    source_overrides: RuntimeSources::new(registry, catalog),
                    store: context.store(store),
                    store_backend: context.store_backend(),
                    tools: context.tools(tools),
                    trace_out,
                    timeout_seconds,
                    max_retries,
                    retry_backoff_ms,
                    hooks: context.hooks()?,
                })
                .await?;
                print_json(&report)?;
            }
        },
        Command::Serve {
            registry,
            catalog,
            store,
            tools,
            stdio,
            host,
            port,
            chat,
        } => {
            let sources = context.runtime_sources(registry, catalog);
            let store = context.store(store);
            let stdio = context.stdio(stdio);
            let host = context.host(host);
            let port = context.port(port);
            let hooks = context.hooks()?;
            let server = RuntimeServer::new(RuntimeServerOptions {
                sources,
                store_path: store,
                store_backend: context.store_backend(),
                tool_overrides: context.tools(tools).load().await?,
                hooks,
                context_policy: context.context_policy(),
                default_agent: context.default_agent(),
                chat: context.chat(chat),
            })
            .await?;
            if stdio {
                serve_stdio(server).await?;
            } else {
                serve_http(server, host, port).await?;
            }
        }
        Command::Tui {
            registry,
            catalog,
            trace,
            store,
            tools,
            deny_high_risk_tools,
            chat,
            timeout_seconds,
            max_retries,
            retry_backoff_ms,
            mouse_capture,
            once,
        } => {
            let sources = context.runtime_sources(registry, catalog);
            let store = context.store(store);
            let execution = context.execution(timeout_seconds, max_retries, retry_backoff_ms);
            run_tui(TuiOptions {
                runtime_sources: sources,
                trace_path: trace,
                store_path: store,
                store_backend: context.store_backend(),
                tool_overrides: context.tools(tools).load().await?,
                allow_high_risk_tools: !deny_high_risk_tools,
                chat: context.chat(chat),
                timeout_seconds: execution.timeout_seconds,
                max_retries: execution.max_retries,
                retry_backoff_ms: execution.retry_backoff_ms,
                hooks: context.hook_specs(),
                context_policy: context.context_policy(),
                default_agent: context.default_agent(),
                mouse_capture,
                once,
            })
            .await?;
        }
        Command::Eval {
            eval_path,
            store,
            tools,
            update_golden,
            from_run,
            out,
            catalog,
            id,
            golden_trace,
        } => {
            let store = context.eval_store(store);
            let catalog = context.catalog(catalog);
            let result = if eval_path.as_str() == "create" || from_run.is_some() {
                create_eval_from_run(
                    from_run.ok_or_else(|| miette!("--from-run is required"))?,
                    store,
                    context.store_backend(),
                    out.ok_or_else(|| miette!("--out is required"))?,
                    catalog.ok_or_else(|| miette!("--catalog is required"))?,
                    id,
                    golden_trace,
                )
                .await?
            } else {
                run_eval_path(
                    eval_path,
                    store,
                    context.store_backend(),
                    context.tools(tools).load().await?,
                    update_golden,
                )
                .await?
            };
            print_json(&result)?;
        }
        Command::DevToolHost => run_dev_tool_host().await?,
        Command::DevMcpServer => run_dev_mcp_server().await?,
        Command::DevScoreHook => run_dev_score_hook().await?,
        Command::ShellToolHost => run_shell_tool_host().await?,
    }
    Ok(())
}
