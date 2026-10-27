use crate::{
    apps::{
        AppTask, AppsError, MergedAppTask,
        queue::{AppTaskQueue, ProcessableTasks, ProcessableTasksError},
        repository::{AppPostgresRepository, QueueLockContext},
    },
    models::AppStatusChangeId,
};
use chrono::{DateTime, Utc};
use domain::app_instance::App;

#[derive(thiserror::Error, Debug)]
pub enum AppPostgresQueueError {
    #[error("{0}")]
    ProcessableTasksError(ProcessableTasksError),
    #[error("Failed to communicate with the database: {0}")]
    DatabaseError(sqlx::Error),
}

impl From<ProcessableTasksError> for AppPostgresQueueError {
    fn from(err: ProcessableTasksError) -> Self {
        Self::ProcessableTasksError(err)
    }
}

impl From<sqlx::Error> for AppPostgresQueueError {
    fn from(err: sqlx::Error) -> Self {
        Self::DatabaseError(err)
    }
}

#[async_trait::async_trait]
impl AppTaskQueue for AppPostgresRepository {
    type LockContext<'lc> = QueueLockContext<'lc>;
    type Error = AppPostgresQueueError;

    async fn enqueue_task(&self, task: AppTask) -> std::result::Result<(), Self::Error> {
        self.enqueue_task(task).await?;
        Ok(())
    }

    async fn peek_result(
        &self,
        status_id: &AppStatusChangeId,
    ) -> Option<std::result::Result<App, AppsError>> {
        self.peek_result(status_id).await
    }

    /// Fetches all [`AppTask`]s that are processable in one batch and returns [`Self::LockContext`]
    /// so that follow-up operations can be performed atomically.
    async fn fetch_processable_tasks<'lc, 'a: 'lc>(
        &'a self,
    ) -> std::result::Result<Option<(ProcessableTasks, Self::LockContext<'lc>)>, Self::Error> {
        // TODO: error
        let Some((tasks, backed_up_apps, tx)) = self.lock_queued_tasks().await.unwrap() else {
            return Ok(None);
        };

        Ok(Some((ProcessableTasks::new(tasks, backed_up_apps)?, tx)))
    }

    async fn store_result<'lc>(
        &self,
        mut lock_context: Self::LockContext<'lc>,
        result: std::result::Result<App, AppsError>,
        _done_timestamp: DateTime<Utc>,
        merged: &MergedAppTask,
    ) -> std::result::Result<(), Self::Error> {
        let MergedAppTask {
            task_to_work_on,
            tasks_to_be_marked_as_done,
            tasks_to_stay_untouched,
        } = merged;

        AppPostgresRepository::store_result(
            &mut lock_context,
            task_to_work_on,
            result,
            tasks_to_be_marked_as_done,
        )
        .await?;

        AppPostgresRepository::move_untouched_tasks_back_into_queue(
            &mut lock_context,
            tasks_to_stay_untouched,
        )
        .await?;

        lock_context.commit().await?;

        Ok(())
    }

    async fn clean_up_done_tasks(
        &self,
        older_than: DateTime<Utc>,
    ) -> std::result::Result<usize, Self::Error> {
        Ok(self.clean_up_done_tasks(older_than).await?)
    }
}
