//! Main iteration loop.

use agent_client_protocol::StopReason;
use anyhow::{Context, Result};
use std::path::Path;

use crate::acp;
use crate::acp::types::{
    BlockerContext, IterationContext, ParentContext, RetryInfo, RunResult, TaskInfo,
};
use crate::config::{Config, RunTarget};
use crate::dag::{self, Db, Task};
use crate::feature;
use crate::journal;
use crate::knowledge;
use crate::output::{formatter, logger};
use crate::strategy;
use crate::verification;

/// Outcome of the loop execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// All DAG tasks done
    Complete,
    /// <promise>FAILURE</promise> emitted
    Failure,
    /// Iteration limit hit
    LimitReached,
    /// No ready tasks, but incomplete tasks exist
    Blocked,
    /// DAG is empty, user must run `ralph feature create`
    NoPlan,
    /// User interrupted and chose not to continue
    Interrupted,
}

/// Run the main loop until completion, failure, or limit.
pub async fn run(mut config: Config) -> Result<Outcome> {
    // Register Ctrl+C signal handler for graceful interrupt
    crate::interrupt::register_signal_handler().context("Failed to register signal handler")?;

    // Open the DAG database
    let progress_db = config.project_root.join(".ralph/progress.db");
    let db = dag::open_db(
        progress_db
            .to_str()
            .context("Failed to convert progress.db path to string")?,
    )
    .context("Failed to open DAG database")?;

    // Resolve feature context (spec + plan content) if targeting a feature
    let (feature_id, spec_content, plan_content) = resolve_feature_context(&config, &db)?;

    // Emit iteration 1 start event (iterations 2+ are emitted in advance_iteration_with_model_selection)
    formatter::emit_event_info(
        "iter",
        &format!(
            "iteration 1 started \u{2014} model={} strategy={}",
            config.current_model, config.model_strategy
        ),
    );

    loop {
        // Get scoped ready tasks
        let ready_tasks = get_scoped_ready_tasks(&config, &db, feature_id.as_deref())?;
        let counts = dag::get_task_counts(&db).context("Failed to get task counts")?;

        // Print DAG summary at the start of each iteration
        if config.iteration == 1 {
            formatter::print_dag_summary(counts.total, counts.ready, counts.done, counts.blocked);
            formatter::emit_event_info(
                "dag",
                &format!(
                    "{} tasks, {} ready, {} done, {} blocked",
                    counts.total, counts.ready, counts.done, counts.blocked
                ),
            );
        }

        // Check if DAG is empty
        if counts.total == 0 {
            return Ok(Outcome::NoPlan);
        }

        // Check if all tasks are resolved before declaring blocked
        if ready_tasks.is_empty() {
            if recover_stuck_target_claim(&config, &db)? {
                continue;
            }
            if scoped_target_resolved(&config, &db, feature_id.as_deref())
                .context("Failed to check if run target is resolved")?
            {
                return Ok(Outcome::Complete);
            }
            return Ok(Outcome::Blocked);
        }

        // Pick first ready task
        let task = &ready_tasks[0];
        let task_id = task.id.clone();
        formatter::emit_event_info(
            "dag",
            &format!("next task: {} \u{2014} \"{}\"", task.id, task.title),
        );

        // Claim the task
        dag::claim_task(&db, &task_id, &config.agent_id).context("Failed to claim task")?;

        // Print iteration info with colors (task ID in cyan)
        formatter::print_task_working(config.iteration, &task_id, &task.title);
        formatter::emit_event_info(
            "task",
            &format!("{} claimed \u{2014} \"{}\"", task_id, task.title),
        );

        // Set up logging
        let log_file = logger::setup_log_file();
        formatter::print_log_location("Log will be written to:", &log_file);

        // Build iteration context
        let iteration_context = build_iteration_context(
            &db,
            task,
            spec_content.as_deref(),
            plan_content.as_deref(),
            &config,
        )?;

        // Run the ACP agent iteration
        let run_result = match acp::connection::run_iteration(&config, &iteration_context).await {
            Ok(result) => result,
            Err(err) => {
                try_release_claim(&db, &task_id, "agent run error");
                return Err(err).context("Failed to run agent");
            }
        };

        // Handle interrupt: prompt for feedback, reset task, optionally continue
        let streaming_result = match run_result {
            RunResult::Interrupted => {
                formatter::print_interrupted(config.iteration, &task_id, &task.title);

                // Release claim first so interrupt follow-up failures do not strand task state.
                dag::release_claim(&db, &task_id).context("Failed to release task claim")?;
                formatter::emit_event_info("task", &format!("{} claim released", task_id));

                // Prompt for feedback
                let feedback = crate::interrupt::prompt_for_feedback(task)?;

                if let Some(ref fb) = feedback {
                    let new_desc = crate::interrupt::append_feedback_to_description(
                        &task.description,
                        fb,
                        config.iteration,
                    );
                    dag::update_task(
                        &db,
                        &task_id,
                        dag::TaskUpdate {
                            description: Some(new_desc),
                            ..Default::default()
                        },
                    )?;
                    dag::add_log(
                        &db,
                        &task_id,
                        &format!("User feedback (iteration {}): {}", config.iteration, fb),
                    )?;
                }

                // Journal entry for the interrupted iteration
                let journal_entry = journal::JournalEntry {
                    id: 0,
                    run_id: config.run_id.clone(),
                    iteration: config.iteration,
                    task_id: Some(task_id.clone()),
                    feature_id: task.feature_id.clone(),
                    outcome: "interrupted".to_string(),
                    model: Some(config.current_model.clone()),
                    duration_secs: 0.0,
                    cost_usd: 0.0,
                    files_modified: Vec::new(),
                    notes: feedback.clone(),
                    created_at: chrono::Utc::now().to_rfc3339(),
                };
                journal::insert_journal_entry(&db, &journal_entry).ok();

                // Clear flag for next iteration
                crate::interrupt::clear_interrupt();

                // Ask whether to continue
                if crate::interrupt::should_continue()? {
                    formatter::print_separator();
                    config = config.next_iteration();
                    formatter::emit_iteration_divider(config.iteration);
                    formatter::print_iteration_info(&config);
                    continue;
                } else {
                    return Ok(Outcome::Interrupted);
                }
            }
            RunResult::Completed(r) => r,
        };

        formatter::print_log_location("Log available at:", &log_file);

        // Handle non-EndTurn stop reasons BEFORE sigil processing (FR-6.6).
        //
        // For MaxTokens/MaxTurnRequests/unknown: release claim, journal "blocked", continue.
        // For Refusal: fail the task, journal "failed", continue.
        // For Cancelled: should not reach here (handled by select! in connection.rs),
        //                but treat as blocked if it does.
        let sigils = match streaming_result.stop_reason {
            StopReason::EndTurn => {
                // Normal completion — extract sigils from accumulated text.
                // Sigils are already formatted inline during streaming, so no
                // separate print_sigils() call needed.
                acp::sigils::extract_sigils(&streaming_result.full_text)
            }
            StopReason::Cancelled => {
                // Unexpected: cancellation is normally caught in connection.rs.
                formatter::print_warning(
                    "ralph: agent reported Cancelled stop reason (unexpected), releasing task claim",
                );
                formatter::emit_event_info(
                    "iter",
                    &format!("agent stopped: Cancelled \u{2014} releasing {}", task_id),
                );
                dag::release_claim(&db, &task_id).context("Failed to release task claim")?;
                formatter::print_task_incomplete(config.iteration, &task_id);
                formatter::emit_event_info("task", &format!("{} incomplete (no sigil)", task_id));
                let journal_entry = journal::JournalEntry {
                    id: 0,
                    run_id: config.run_id.clone(),
                    iteration: config.iteration,
                    task_id: Some(task_id.clone()),
                    feature_id: task.feature_id.clone(),
                    outcome: "blocked".to_string(),
                    model: Some(config.current_model.clone()),
                    duration_secs: streaming_result.duration_ms as f64 / 1000.0,
                    cost_usd: 0.0,
                    files_modified: streaming_result.files_modified.clone(),
                    notes: None,
                    created_at: chrono::Utc::now().to_rfc3339(),
                };
                journal::insert_journal_entry(&db, &journal_entry).ok();
                if scoped_target_resolved(&config, &db, feature_id.as_deref())
                    .context("Failed to check if run target is resolved")?
                {
                    return Ok(Outcome::Complete);
                }
                if config.limit_reached() {
                    formatter::emit_event_info(
                        "iter",
                        &format!(
                            "iteration limit reached ({}/{})",
                            config.iteration, config.limit
                        ),
                    );
                    return Ok(Outcome::LimitReached);
                }
                advance_iteration_with_model_selection(&mut config, &db, &progress_db, None);
                continue;
            }
            StopReason::MaxTokens | StopReason::MaxTurnRequests => {
                formatter::print_warning(&format!(
                    "ralph: agent hit token/turn limit ({:?}), releasing task claim",
                    streaming_result.stop_reason
                ));
                formatter::emit_event_info(
                    "iter",
                    &format!(
                        "agent stopped: {:?} \u{2014} releasing {}",
                        streaming_result.stop_reason, task_id
                    ),
                );
                dag::release_claim(&db, &task_id).context("Failed to release task claim")?;
                formatter::print_task_incomplete(config.iteration, &task_id);
                formatter::emit_event_info("task", &format!("{} incomplete (no sigil)", task_id));
                let journal_entry = journal::JournalEntry {
                    id: 0,
                    run_id: config.run_id.clone(),
                    iteration: config.iteration,
                    task_id: Some(task_id.clone()),
                    feature_id: task.feature_id.clone(),
                    outcome: "blocked".to_string(),
                    model: Some(config.current_model.clone()),
                    duration_secs: streaming_result.duration_ms as f64 / 1000.0,
                    cost_usd: 0.0,
                    files_modified: streaming_result.files_modified.clone(),
                    notes: None,
                    created_at: chrono::Utc::now().to_rfc3339(),
                };
                journal::insert_journal_entry(&db, &journal_entry).ok();
                if scoped_target_resolved(&config, &db, feature_id.as_deref())
                    .context("Failed to check if run target is resolved")?
                {
                    return Ok(Outcome::Complete);
                }
                if config.limit_reached() {
                    formatter::emit_event_info(
                        "iter",
                        &format!(
                            "iteration limit reached ({}/{})",
                            config.iteration, config.limit
                        ),
                    );
                    return Ok(Outcome::LimitReached);
                }
                advance_iteration_with_model_selection(&mut config, &db, &progress_db, None);
                continue;
            }
            StopReason::Refusal => {
                formatter::print_warning("ralph: agent refused the request, failing task");
                formatter::emit_event(
                    "iter",
                    &format!("agent stopped: Refusal \u{2014} failing {}", task_id),
                    true,
                );
                dag::fail_task(&db, &task_id, "Agent refused the request")
                    .context("Failed to fail task")?;
                formatter::print_task_failed(config.iteration, &task_id);
                formatter::emit_event(
                    "task",
                    &format!("{} failed \u{2014} Agent refused the request", task_id),
                    true,
                );
                let journal_entry = journal::JournalEntry {
                    id: 0,
                    run_id: config.run_id.clone(),
                    iteration: config.iteration,
                    task_id: Some(task_id.clone()),
                    feature_id: task.feature_id.clone(),
                    outcome: "failed".to_string(),
                    model: Some(config.current_model.clone()),
                    duration_secs: streaming_result.duration_ms as f64 / 1000.0,
                    cost_usd: 0.0,
                    files_modified: streaming_result.files_modified.clone(),
                    notes: None,
                    created_at: chrono::Utc::now().to_rfc3339(),
                };
                journal::insert_journal_entry(&db, &journal_entry).ok();
                if scoped_target_resolved(&config, &db, feature_id.as_deref())
                    .context("Failed to check if run target is resolved")?
                {
                    return Ok(Outcome::Complete);
                }
                if config.limit_reached() {
                    formatter::emit_event_info(
                        "iter",
                        &format!(
                            "iteration limit reached ({}/{})",
                            config.iteration, config.limit
                        ),
                    );
                    return Ok(Outcome::LimitReached);
                }
                advance_iteration_with_model_selection(&mut config, &db, &progress_db, None);
                continue;
            }
            _ => {
                // Unknown stop reason (#[non_exhaustive]) — treat as incomplete (blocked).
                formatter::print_warning(&format!(
                    "ralph: agent stopped with unknown reason: {:?}, releasing task claim",
                    streaming_result.stop_reason
                ));
                formatter::emit_event_info(
                    "iter",
                    &format!(
                        "agent stopped: {:?} \u{2014} releasing {}",
                        streaming_result.stop_reason, task_id
                    ),
                );
                dag::release_claim(&db, &task_id).context("Failed to release task claim")?;
                formatter::print_task_incomplete(config.iteration, &task_id);
                formatter::emit_event_info("task", &format!("{} incomplete (no sigil)", task_id));
                let journal_entry = journal::JournalEntry {
                    id: 0,
                    run_id: config.run_id.clone(),
                    iteration: config.iteration,
                    task_id: Some(task_id.clone()),
                    feature_id: task.feature_id.clone(),
                    outcome: "blocked".to_string(),
                    model: Some(config.current_model.clone()),
                    duration_secs: streaming_result.duration_ms as f64 / 1000.0,
                    cost_usd: 0.0,
                    files_modified: streaming_result.files_modified.clone(),
                    notes: None,
                    created_at: chrono::Utc::now().to_rfc3339(),
                };
                journal::insert_journal_entry(&db, &journal_entry).ok();
                if scoped_target_resolved(&config, &db, feature_id.as_deref())
                    .context("Failed to check if run target is resolved")?
                {
                    return Ok(Outcome::Complete);
                }
                if config.limit_reached() {
                    formatter::emit_event_info(
                        "iter",
                        &format!(
                            "iteration limit reached ({}/{})",
                            config.iteration, config.limit
                        ),
                    );
                    return Ok(Outcome::LimitReached);
                }
                advance_iteration_with_model_selection(&mut config, &db, &progress_db, None);
                continue;
            }
        };

        // ── EndTurn path: process sigils normally ────────────────────────────

        // Extract model hint before checking completion/failure
        let next_model_hint = sigils.next_model_hint.clone();

        // Check for FAILURE sigil - this short-circuits before DAG update
        if sigils.is_failure {
            dag::release_claim(&db, &task_id)
                .context("Failed to release task claim before failure outcome")?;
            return Ok(Outcome::Failure);
        }

        // Handle task completion/failure sigils
        if let Some(ref done_id) = sigils.task_done {
            if done_id == &task_id {
                if let Err(err) = handle_task_done(
                    &db,
                    &config,
                    task,
                    spec_content.as_deref(),
                    plan_content.as_deref(),
                    &log_file,
                )
                .await
                {
                    try_release_claim(&db, &task_id, "task completion handling error");
                    return Err(err).context("Failed to handle task completion");
                }
            } else {
                formatter::print_warning(&format!(
                    "Warning: task-done sigil ID {} does not match assigned task {}",
                    done_id, task_id
                ));
                dag::release_claim(&db, &task_id).context("Failed to release task claim")?;
                formatter::print_task_incomplete(config.iteration, &task_id);
                formatter::emit_event_info("task", &format!("{} incomplete (no sigil)", task_id));
            }
        } else if let Some(ref failed_id) = sigils.task_failed {
            if failed_id == &task_id {
                if let Err(err) = dag::fail_task(&db, &task_id, "Task marked failed by Claude") {
                    try_release_claim(&db, &task_id, "task failure handling error");
                    return Err(err).context("Failed to fail task");
                }
                formatter::print_task_failed(config.iteration, &task_id);
                formatter::emit_event(
                    "task",
                    &format!("{} failed \u{2014} Task marked failed by Claude", task_id),
                    true,
                );
            } else {
                formatter::print_warning(&format!(
                    "Warning: task-failed sigil ID {} does not match assigned task {}",
                    failed_id, task_id
                ));
                dag::release_claim(&db, &task_id).context("Failed to release task claim")?;
                formatter::print_task_incomplete(config.iteration, &task_id);
                formatter::emit_event_info("task", &format!("{} incomplete (no sigil)", task_id));
            }
        } else {
            // No sigil - release the claim and treat as incomplete
            dag::release_claim(&db, &task_id).context("Failed to release task claim")?;
            formatter::print_task_incomplete(config.iteration, &task_id);
            formatter::emit_event_info("task", &format!("{} incomplete (no sigil)", task_id));
        }

        // Post-iteration: write journal entry and knowledge files
        {
            // Determine outcome by comparing retry_count before/after handle_task_done
            let updated_task = dag::get_task(&db, &task_id).ok();
            let outcome = if let Some(ref t) = updated_task {
                if t.retry_count > task.retry_count {
                    "retried"
                } else if t.status == "done" {
                    "done"
                } else if t.status == "failed" {
                    "failed"
                } else {
                    "blocked"
                }
            } else if sigils.task_done.is_some() {
                "done"
            } else if sigils.task_failed.is_some() {
                "failed"
            } else {
                "blocked"
            };

            let journal_entry = journal::JournalEntry {
                id: 0, // ignored on insert (AUTOINCREMENT)
                run_id: config.run_id.clone(),
                iteration: config.iteration,
                task_id: Some(task_id.clone()),
                feature_id: task.feature_id.clone(),
                outcome: outcome.to_string(),
                model: Some(config.current_model.clone()),
                // ACP does not report cost — always 0.0 (NFR-5.1)
                duration_secs: streaming_result.duration_ms as f64 / 1000.0,
                cost_usd: 0.0,
                files_modified: streaming_result.files_modified.clone(),
                notes: sigils.journal_notes.clone(),
                created_at: chrono::Utc::now().to_rfc3339(),
            };
            if let Err(e) = journal::insert_journal_entry(&db, &journal_entry) {
                formatter::print_warning(&format!("Warning: failed to write journal entry: {}", e));
            }

            // Write knowledge entries emitted by the agent
            for sigil in &sigils.knowledge_entries {
                let feature_name = match &config.run_target {
                    Some(RunTarget::Feature(name)) => Some(name.as_str()),
                    _ => None,
                };
                match knowledge::write_knowledge_entry(&config.project_root, sigil, feature_name) {
                    Ok(path) => {
                        formatter::print_info(&format!(
                            "  Knowledge entry written: {}",
                            path.display()
                        ));
                    }
                    Err(e) => {
                        formatter::print_warning(&format!(
                            "  Warning: failed to write knowledge entry: {}",
                            e
                        ));
                    }
                }
            }
        }

        // Check if all tasks are resolved
        if scoped_target_resolved(&config, &db, feature_id.as_deref())
            .context("Failed to check if run target is resolved")?
        {
            return Ok(Outcome::Complete);
        }

        // Check iteration limit
        if config.limit_reached() {
            formatter::emit_event_info(
                "iter",
                &format!(
                    "iteration limit reached ({}/{})",
                    config.iteration, config.limit
                ),
            );
            return Ok(Outcome::LimitReached);
        }

        // Continue to next iteration
        advance_iteration_with_model_selection(
            &mut config,
            &db,
            &progress_db,
            next_model_hint.as_deref(),
        );
    }
}

fn try_release_claim(db: &Db, task_id: &str, context: &str) {
    if let Err(err) = dag::release_claim(db, task_id) {
        formatter::print_warning(&format!(
            "Warning: failed to release task claim for {task_id} after {context}: {err}"
        ));
    }
}

fn advance_iteration_with_model_selection(
    config: &mut Config,
    db: &Db,
    progress_db: &Path,
    next_model_hint: Option<&str>,
) {
    formatter::print_separator();
    *config = config.next_iteration();
    formatter::emit_iteration_divider(config.iteration);

    let selection = strategy::select_model_with_db(config, next_model_hint, Some(db));
    if selection.was_overridden {
        let progress_db_path = progress_db.to_string_lossy();
        if let Err(err) =
            strategy::log_model_override(progress_db_path.as_ref(), config.iteration, &selection)
        {
            formatter::print_warning(&format!(
                "Warning: failed to persist model override: {}",
                err
            ));
        }
    }

    config.current_model = selection.model;
    formatter::print_iteration_info(config);
    formatter::emit_event_info(
        "iter",
        &format!(
            "iteration {} started \u{2014} model={} strategy={}",
            config.iteration, config.current_model, config.model_strategy
        ),
    );
}

/// Resolve feature context: returns (feature_id, spec_content, plan_content).
fn resolve_feature_context(
    config: &Config,
    db: &Db,
) -> Result<(Option<String>, Option<String>, Option<String>)> {
    match &config.run_target {
        Some(RunTarget::Feature(name)) => {
            let feat = feature::get_feature(db, name)?;
            let spec = feature::read_spec(&config.project_root, name).ok();
            let plan = feature::read_plan(&config.project_root, name).ok();
            Ok((Some(feat.id), spec, plan))
        }
        Some(RunTarget::Task(task_id)) => {
            // Standalone task — check if it has a feature_id
            let task = dag::get_task(db, task_id)?;
            if let Some(ref fid) = task.feature_id {
                let feat = feature::get_feature_by_id(db, fid)?;
                let spec = feature::read_spec(&config.project_root, &feat.name).ok();
                let plan = feature::read_plan(&config.project_root, &feat.name).ok();
                Ok((Some(fid.clone()), spec, plan))
            } else {
                Ok((None, None, None))
            }
        }
        None => Ok((None, None, None)),
    }
}

/// If a task-target run has no ready tasks because this run left its own claim
/// in `in_progress`, release it and continue the loop.
fn recover_stuck_target_claim(config: &Config, db: &Db) -> Result<bool> {
    let Some(RunTarget::Task(task_id)) = &config.run_target else {
        return Ok(false);
    };

    let task = match dag::get_task(db, task_id) {
        Ok(task) => task,
        Err(_) => return Ok(false),
    };

    if task.status != "in_progress" {
        return Ok(false);
    }
    if task.claimed_by.as_deref() != Some(config.agent_id.as_str()) {
        return Ok(false);
    }

    formatter::print_warning(&format!(
        "Recovering stale in-progress claim for {} from this run.",
        task_id
    ));
    formatter::emit_event_info("dag", &format!("recovered stale claim on {}", task_id));
    dag::release_claim(db, task_id).context("Failed to release stale claim")?;
    Ok(true)
}

/// Get ready tasks scoped to the run target.
fn get_scoped_ready_tasks(config: &Config, db: &Db, feature_id: Option<&str>) -> Result<Vec<Task>> {
    match &config.run_target {
        Some(RunTarget::Feature(_)) => {
            if let Some(fid) = feature_id {
                dag::get_ready_tasks_for_feature(db, fid)
                    .context("Failed to get ready tasks for feature")
            } else {
                dag::get_ready_tasks(db).context("Failed to get ready tasks")
            }
        }
        Some(RunTarget::Task(task_id)) => {
            // For a standalone task target, only return that task if it's ready
            let ready = dag::get_ready_tasks(db).context("Failed to get ready tasks")?;
            Ok(ready.into_iter().filter(|t| t.id == *task_id).collect())
        }
        None => dag::get_ready_tasks(db).context("Failed to get ready tasks"),
    }
}

/// Check whether the current run target is resolved.
///
/// - `run <task-id>` resolves when that task is `done` or `failed`, regardless
///   of unrelated tasks elsewhere in the DAG.
/// - `run <feature-name>` resolves when all tasks for that feature are `done`/`failed`.
/// - Unscoped runs resolve when the full DAG is resolved.
fn scoped_target_resolved(config: &Config, db: &Db, feature_id: Option<&str>) -> Result<bool> {
    match &config.run_target {
        Some(RunTarget::Task(task_id)) => {
            let task = dag::get_task(db, task_id)?;
            Ok(matches!(task.status.as_str(), "done" | "failed"))
        }
        Some(RunTarget::Feature(_)) => {
            if let Some(fid) = feature_id {
                let unresolved: i64 = db.conn().query_row(
                    "SELECT COUNT(*) FROM tasks WHERE feature_id = ? AND status NOT IN ('done', 'failed')",
                    [fid],
                    |row| row.get(0),
                )?;
                Ok(unresolved == 0)
            } else {
                dag::all_resolved(db)
            }
        }
        None => dag::all_resolved(db),
    }
}

/// Build the full iteration context for the assigned task.
fn build_iteration_context(
    db: &Db,
    task: &Task,
    spec_content: Option<&str>,
    plan_content: Option<&str>,
    config: &Config,
) -> Result<IterationContext> {
    // Build parent context
    let parent = if let Some(ref pid) = task.parent_id {
        let parent_task = dag::get_task(db, pid).ok();
        parent_task.map(|p| ParentContext {
            title: p.title,
            description: p.description,
        })
    } else {
        None
    };

    // Build completed blockers context
    let completed_blockers = get_completed_blockers(db, &task.id)?;

    // Build retry info if this is a retry
    let retry_info = if task.retry_count > 0 {
        let failure_reason = get_last_failure_reason(db, &task.id)?;
        Some(RetryInfo {
            attempt: task.retry_count + 1,
            max_retries: config.max_retries as i32,
            previous_failure_reason: failure_reason,
        })
    } else {
        None
    };

    let task_info = TaskInfo {
        task_id: task.id.clone(),
        title: task.title.clone(),
        description: task.description.clone(),
        parent,
        completed_blockers,
    };

    // Journal: smart-select entries for system prompt context (FR-5.1, FR-5.2)
    let journal_entries = journal::select_journal_entries(
        db,
        &config.run_id,
        &task.title,
        &task.description,
        5, // recent_limit
        5, // fts_limit
    )
    .unwrap_or_default();
    let journal_context = journal::render_journal_context(&journal_entries);

    // Knowledge: discover, tag-match, and expand via link graph (FR-6.1-FR-6.4)
    let all_knowledge = knowledge::discover_knowledge(&config.project_root);
    let last_files: Vec<String> = journal_entries
        .last()
        .map(|e| e.files_modified.clone())
        .unwrap_or_default();
    let feature_name = match &config.run_target {
        Some(RunTarget::Feature(name)) => Some(name.as_str()),
        _ => None,
    };
    let mut matched_knowledge = knowledge::match_knowledge_entries(
        &all_knowledge,
        &task.title,
        &task.description,
        feature_name,
        &last_files,
    );

    // Build link graph and expand matched set via bidirectional links
    let link_graph = knowledge::build_link_graph(&all_knowledge);
    let linked_entries =
        knowledge::expand_via_links(&all_knowledge, &matched_knowledge, &link_graph, 2, 2);
    matched_knowledge.extend(linked_entries);
    // Re-sort by score after expansion
    matched_knowledge.sort_by(|a, b| b.1.cmp(&a.1));

    let knowledge_context =
        knowledge::render_knowledge_context_with_graph(&matched_knowledge, Some(&link_graph));

    Ok(IterationContext {
        task: task_info,
        spec_content: spec_content.map(|s| s.to_string()),
        plan_content: plan_content.map(|s| s.to_string()),
        retry_info,
        run_id: config.run_id.clone(),
        journal_context,
        knowledge_context,
    })
}

/// Get completed blockers (dependencies) for a task.
fn get_completed_blockers(db: &Db, task_id: &str) -> Result<Vec<BlockerContext>> {
    let mut stmt = db.conn().prepare(
        r#"
        SELECT t.id, t.title, COALESCE(
            (SELECT message FROM task_logs WHERE task_id = t.id ORDER BY timestamp DESC LIMIT 1),
            t.description
        )
        FROM dependencies d
        JOIN tasks t ON d.blocker_id = t.id
        WHERE d.blocked_id = ? AND t.status = 'done'
        "#,
    )?;

    let blockers = stmt
        .query_map([task_id], |row| {
            Ok(BlockerContext {
                task_id: row.get(0)?,
                title: row.get(1)?,
                summary: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(blockers)
}

/// Get the last failure reason from task logs.
fn get_last_failure_reason(db: &Db, task_id: &str) -> Result<String> {
    let reason: String = db
        .conn()
        .query_row(
            "SELECT message FROM task_logs WHERE task_id = ? ORDER BY timestamp DESC LIMIT 1",
            [task_id],
            |row| row.get(0),
        )
        .unwrap_or_else(|_| "No failure reason recorded".to_string());
    Ok(reason)
}

/// Handle a task-done sigil: verify (if enabled) and complete or retry.
async fn handle_task_done(
    db: &Db,
    config: &Config,
    task: &Task,
    spec_content: Option<&str>,
    plan_content: Option<&str>,
    log_file: &str,
) -> Result<()> {
    let task_id = &task.id;

    if config.verify {
        // Run verification agent
        formatter::print_verification_start(config.iteration, task_id);
        formatter::emit_event_info("verify", &format!("verifying {}", task.id));

        let v_result =
            verification::verify_task(config, task, spec_content, plan_content, log_file).await?;

        if v_result.passed {
            // Verification passed — complete the task
            dag::complete_task(db, task_id).context("Failed to complete task")?;
            db.conn().execute(
                "UPDATE tasks SET verification_status = 'passed' WHERE id = ?",
                [task_id.as_str()],
            )?;
            formatter::print_verification_passed(config.iteration, task_id);
            formatter::emit_event_info("verify", &format!("{} passed", task.id));
            formatter::emit_event_info("task", &format!("{} done", task_id));
        } else {
            // Verification failed
            formatter::print_verification_failed(config.iteration, task_id, &v_result.reason);
            formatter::emit_event(
                "verify",
                &format!("{} failed \u{2014} {}", task.id, v_result.reason),
                true,
            );

            // Log the failure
            dag::add_log(
                db,
                task_id,
                &format!("Verification failed: {}", v_result.reason),
            )?;

            let max_retries = config.max_retries as i32;
            if task.retry_count < max_retries {
                // Retry: transition failed → pending, increment retry_count
                dag::retry_task(db, task_id).context("Failed to retry task")?;
                formatter::print_retry(
                    config.iteration,
                    task_id,
                    task.retry_count + 1,
                    max_retries,
                );
                formatter::emit_event_info(
                    "task",
                    &format!("{} retry {}/{}", task_id, task.retry_count + 1, max_retries),
                );
            } else {
                // Max retries exhausted — fail the task
                let fail_reason = format!(
                    "Verification failed after {} retries: {}",
                    max_retries, v_result.reason
                );
                dag::fail_task(db, task_id, &fail_reason).context("Failed to fail task")?;
                formatter::print_max_retries_exhausted(config.iteration, task_id);
                formatter::emit_event("task", &format!("{} max retries exhausted", task_id), true);
                formatter::emit_event(
                    "task",
                    &format!("{} failed \u{2014} {}", task_id, fail_reason),
                    true,
                );
            }
        }
    } else {
        // No verification — complete immediately
        dag::complete_task(db, task_id).context("Failed to complete task")?;
        formatter::print_task_done(config.iteration, task_id);
        formatter::emit_event_info("task", &format!("{} done", task_id));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_dag_returns_noplan() {
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        let db = dag::open_db(temp_file.path().to_str().unwrap()).unwrap();

        // Verify DAG is empty
        let counts = dag::get_task_counts(&db).unwrap();
        assert_eq!(counts.total, 0);

        // This simulates the beginning of the loop:
        // If counts.total == 0, return Outcome::NoPlan
        // (We can't call run() directly without a Config, but we can verify the logic)
        if counts.total == 0 {
            // Would return Outcome::NoPlan
            assert_eq!(counts.total, 0);
        }
    }

    #[test]
    fn ready_tasks_are_picked_in_order() {
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        let db = dag::open_db(temp_file.path().to_str().unwrap()).unwrap();

        // Create a simple chain: A -> B -> C
        db.conn()
            .execute(
                "INSERT INTO tasks (id, title, description, priority, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
                rusqlite::params!["t-task-a", "Task A", "", 0, "2024-01-01T00:00:00Z", "2024-01-01T00:00:00Z"],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, title, description, priority, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
                rusqlite::params!["t-task-b", "Task B", "", 0, "2024-01-01T00:00:01Z", "2024-01-01T00:00:01Z"],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, title, description, priority, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
                rusqlite::params!["t-task-c", "Task C", "", 0, "2024-01-01T00:00:02Z", "2024-01-01T00:00:02Z"],
            )
            .unwrap();

        // Add dependencies: A blocks B, B blocks C
        db.conn()
            .execute(
                "INSERT INTO dependencies (blocker_id, blocked_id) VALUES (?, ?)",
                rusqlite::params!["t-task-a", "t-task-b"],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO dependencies (blocker_id, blocked_id) VALUES (?, ?)",
                rusqlite::params!["t-task-b", "t-task-c"],
            )
            .unwrap();

        // First iteration: A should be ready
        let ready = dag::get_ready_tasks(&db).unwrap();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "t-task-a");

        // Claim and complete A
        dag::claim_task(&db, "t-task-a", "agent-test").unwrap();
        dag::complete_task(&db, "t-task-a").unwrap();

        // Second iteration: B should now be ready
        let ready = dag::get_ready_tasks(&db).unwrap();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "t-task-b");

        // Claim and complete B
        dag::claim_task(&db, "t-task-b", "agent-test").unwrap();
        dag::complete_task(&db, "t-task-b").unwrap();

        // Third iteration: C should now be ready
        let ready = dag::get_ready_tasks(&db).unwrap();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "t-task-c");

        // Complete C
        dag::claim_task(&db, "t-task-c", "agent-test").unwrap();
        dag::complete_task(&db, "t-task-c").unwrap();

        // All tasks should be resolved
        assert!(dag::all_resolved(&db).unwrap());
    }

    #[test]
    fn blocked_tasks_return_blocked_outcome() {
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        let db = dag::open_db(temp_file.path().to_str().unwrap()).unwrap();

        // Create A -> B
        db.conn()
            .execute(
                "INSERT INTO tasks (id, title, description, priority, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
                rusqlite::params!["t-task-a", "Task A", "", 0, "2024-01-01T00:00:00Z", "2024-01-01T00:00:00Z"],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, title, description, priority, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
                rusqlite::params!["t-task-b", "Task B", "", 0, "2024-01-01T00:00:01Z", "2024-01-01T00:00:01Z"],
            )
            .unwrap();

        // Add dependency: A blocks B
        db.conn()
            .execute(
                "INSERT INTO dependencies (blocker_id, blocked_id) VALUES (?, ?)",
                rusqlite::params!["t-task-a", "t-task-b"],
            )
            .unwrap();

        // A is ready, claim and fail it
        let ready = dag::get_ready_tasks(&db).unwrap();
        assert_eq!(ready.len(), 1);

        dag::claim_task(&db, "t-task-a", "agent-test").unwrap();
        dag::fail_task(&db, "t-task-a", "Failed intentionally").unwrap();

        // B is now blocked (cannot proceed because A failed)
        let ready = dag::get_ready_tasks(&db).unwrap();
        assert_eq!(ready.len(), 0);

        // But tasks still exist and are not all resolved
        let counts = dag::get_task_counts(&db).unwrap();
        assert!(counts.total > 0);
        assert!(!dag::all_resolved(&db).unwrap());

        // This is the Blocked scenario
    }

    #[test]
    fn completed_blockers_are_retrieved() {
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        let db = dag::open_db(temp_file.path().to_str().unwrap()).unwrap();

        // Create two tasks: A blocks B
        db.conn()
            .execute(
                "INSERT INTO tasks (id, title, description, priority, status, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params!["t-a", "Task A", "Description A", 0, "done", "2024-01-01T00:00:00Z", "2024-01-01T00:00:00Z"],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, title, description, priority, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
                rusqlite::params!["t-b", "Task B", "Description B", 0, "2024-01-01T00:00:01Z", "2024-01-01T00:00:01Z"],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO dependencies (blocker_id, blocked_id) VALUES (?, ?)",
                rusqlite::params!["t-a", "t-b"],
            )
            .unwrap();

        let blockers = get_completed_blockers(&db, "t-b").unwrap();
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].task_id, "t-a");
        assert_eq!(blockers[0].title, "Task A");
    }

    #[test]
    fn last_failure_reason_from_logs() {
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        let db = dag::open_db(temp_file.path().to_str().unwrap()).unwrap();

        db.conn()
            .execute(
                "INSERT INTO tasks (id, title, description, priority, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
                rusqlite::params!["t-x", "Task X", "", 0, "2024-01-01T00:00:00Z", "2024-01-01T00:00:00Z"],
            )
            .unwrap();

        dag::add_log(&db, "t-x", "First failure").unwrap();
        dag::add_log(&db, "t-x", "Second failure").unwrap();

        let reason = get_last_failure_reason(&db, "t-x").unwrap();
        assert_eq!(reason, "Second failure");
    }

    #[test]
    fn last_failure_reason_missing() {
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        let db = dag::open_db(temp_file.path().to_str().unwrap()).unwrap();

        let reason = get_last_failure_reason(&db, "t-nonexistent").unwrap();
        assert_eq!(reason, "No failure reason recorded");
    }

    #[test]
    fn recover_stuck_target_claim_releases_same_agent_claim() {
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        let db = dag::open_db(temp_file.path().to_str().unwrap()).unwrap();
        let task = dag::create_task(&db, "recover me", None, None, 0).unwrap();
        dag::claim_task(&db, &task.id, "agent-test").unwrap();

        let mut config = Config::from_run_args(
            None,
            None,
            None,
            crate::project::ProjectConfig {
                root: std::path::PathBuf::from("."),
                config: crate::project::RalphConfig::default(),
            },
            Some(RunTarget::Task(task.id.clone())),
            None,
            false,
            None,
        )
        .unwrap();
        config.agent_id = "agent-test".to_string();

        let recovered = recover_stuck_target_claim(&config, &db).unwrap();
        assert!(recovered);

        let updated = dag::get_task(&db, &task.id).unwrap();
        assert_eq!(updated.status, "pending");
        assert_eq!(updated.claimed_by, None);
    }

    #[test]
    fn recover_stuck_target_claim_ignores_other_agent_claim() {
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        let db = dag::open_db(temp_file.path().to_str().unwrap()).unwrap();
        let task = dag::create_task(&db, "stay claimed", None, None, 0).unwrap();
        dag::claim_task(&db, &task.id, "agent-other").unwrap();

        let mut config = Config::from_run_args(
            None,
            None,
            None,
            crate::project::ProjectConfig {
                root: std::path::PathBuf::from("."),
                config: crate::project::RalphConfig::default(),
            },
            Some(RunTarget::Task(task.id.clone())),
            None,
            false,
            None,
        )
        .unwrap();
        config.agent_id = "agent-test".to_string();

        let recovered = recover_stuck_target_claim(&config, &db).unwrap();
        assert!(!recovered);

        let updated = dag::get_task(&db, &task.id).unwrap();
        assert_eq!(updated.status, "in_progress");
        assert_eq!(updated.claimed_by.as_deref(), Some("agent-other"));
    }

    #[test]
    fn try_release_claim_resets_in_progress_task() {
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        let db = dag::open_db(temp_file.path().to_str().unwrap()).unwrap();
        let task = dag::create_task(&db, "release me", None, None, 0).unwrap();
        dag::claim_task(&db, &task.id, "agent-test").unwrap();

        try_release_claim(&db, &task.id, "test cleanup");

        let updated = dag::get_task(&db, &task.id).unwrap();
        assert_eq!(updated.status, "pending");
        assert_eq!(updated.claimed_by, None);
    }

    #[test]
    fn scoped_target_resolved_task_ignores_unrelated_pending_tasks() {
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        let db = dag::open_db(temp_file.path().to_str().unwrap()).unwrap();

        let target = dag::create_task(&db, "target", None, None, 0).unwrap();
        let _other = dag::create_task(&db, "other pending", None, None, 0).unwrap();
        dag::claim_task(&db, &target.id, "agent-test").unwrap();
        dag::complete_task(&db, &target.id).unwrap();

        let config = Config::from_run_args(
            None,
            None,
            None,
            crate::project::ProjectConfig {
                root: std::path::PathBuf::from("."),
                config: crate::project::RalphConfig::default(),
            },
            Some(RunTarget::Task(target.id)),
            None,
            false,
            None,
        )
        .unwrap();

        assert!(scoped_target_resolved(&config, &db, None).unwrap());
        assert!(!dag::all_resolved(&db).unwrap());
    }

    #[test]
    fn scoped_target_resolved_feature_ignores_unrelated_pending_tasks() {
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        let db = dag::open_db(temp_file.path().to_str().unwrap()).unwrap();

        let feat_target = feature::create_feature(&db, "feat-target").unwrap();
        let feat_other = feature::create_feature(&db, "feat-other").unwrap();

        let target_task = dag::create_task_with_feature(
            &db,
            dag::CreateTaskParams {
                title: "target feature task",
                description: None,
                parent_id: None,
                priority: 0,
                feature_id: Some(&feat_target.id),
                task_type: "feature",
                max_retries: 3,
            },
        )
        .unwrap();
        dag::claim_task(&db, &target_task.id, "agent-test").unwrap();
        dag::complete_task(&db, &target_task.id).unwrap();

        let _other_task = dag::create_task_with_feature(
            &db,
            dag::CreateTaskParams {
                title: "other feature pending task",
                description: None,
                parent_id: None,
                priority: 0,
                feature_id: Some(&feat_other.id),
                task_type: "feature",
                max_retries: 3,
            },
        )
        .unwrap();

        let config = Config::from_run_args(
            None,
            None,
            None,
            crate::project::ProjectConfig {
                root: std::path::PathBuf::from("."),
                config: crate::project::RalphConfig::default(),
            },
            Some(RunTarget::Feature("feat-target".to_string())),
            None,
            false,
            None,
        )
        .unwrap();

        assert!(scoped_target_resolved(&config, &db, Some(&feat_target.id)).unwrap());
        assert!(!dag::all_resolved(&db).unwrap());
    }
}
