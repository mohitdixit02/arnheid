//! tokio-cron-scheduler wiring.

pub mod jobs;

use crate::state::AppState;
use anyhow::Result;
use tokio_cron_scheduler::{Job, JobScheduler};

/// Build and start the scheduler.
pub async fn start(state: AppState) -> Result<JobScheduler> {
    let sched = JobScheduler::new().await?;

    // Graph builder — every 6 hours.
    {
        let state = state.clone();
        let schedule = state.config.graph_cron_schedule.clone();
        sched
            .add(Job::new_async(schedule.as_str(), move |_uuid, _l| {
                let state = state.clone();
                Box::pin(async move { jobs::graph_builder(&state).await })
            })?)
            .await?;
    }

    // Cleanup — daily.
    {
        let state = state.clone();
        let schedule = state.config.cleanup_cron_schedule.clone();
        sched
            .add(Job::new_async(schedule.as_str(), move |_uuid, _l| {
                let state = state.clone();
                Box::pin(async move { jobs::cleanup(&state).await })
            })?)
            .await?;
    }

    // Health + retry — every 15 minutes.
    {
        let state = state.clone();
        let schedule = state.config.health_cron_schedule.clone();
        sched
            .add(Job::new_async(schedule.as_str(), move |_uuid, _l| {
                let state = state.clone();
                Box::pin(async move { jobs::health_and_retry(&state).await })
            })?)
            .await?;
    }

    // CockroachDB Cloud Monitor — hourly (configurable).
    {
        let state = state.clone();
        let schedule = state.config.cockroach_cloud_monitor_cron.clone();
        sched
            .add(Job::new_async(schedule.as_str(), move |_uuid, _l| {
                let state = state.clone();
                Box::pin(async move { jobs::cockroach_cloud_monitor(&state).await })
            })?)
            .await?;
    }

    sched.start().await?;
    tracing::info!("cron scheduler started");
    println!("[info] cron scheduler started");
    Ok(sched)
}
