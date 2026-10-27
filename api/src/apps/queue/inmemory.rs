use crate::{
    apps::{
        AppTask, AppsError, MergedAppTask,
        queue::{AppTaskQueue, ProcessableTasks, ProcessableTasksError},
    },
    models::AppStatusChangeId,
};
use chrono::{DateTime, Utc};
use domain::app_instance::App;
use std::collections::VecDeque;
use tokio::sync::Mutex;
use uuid::Uuid;

enum AppTaskStatus {
    New,
    InProcess(Uuid),
    Done((DateTime<Utc>, std::result::Result<App, AppsError>)),
}

pub struct InMemoryQueue {
    queue: Mutex<VecDeque<(AppTask, AppTaskStatus)>>,
}

impl InMemoryQueue {
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum InMemoryQueueError {
    #[error("Cannot fetch app tasks due to fail tasks aggregations: {0}")]
    FailedToFetchTasks(ProcessableTasksError),
    #[error("Cannot update {status_id} in queue which should be present")]
    CannotFindAppStatusChangeId { status_id: AppStatusChangeId },
    #[error("Back-up & restore is not supported by in-memory app queue processing.")]
    BackUpAndRestoreIsNotSupported,
}

#[async_trait::async_trait]
impl AppTaskQueue for InMemoryQueue {
    type LockContext<'lc> = Uuid;
    type Error = InMemoryQueueError;

    async fn enqueue_task(&self, task: AppTask) -> std::result::Result<(), Self::Error> {
        if matches!(
            task,
            AppTask::MovePayloadToBackUpAndDeleteFromInfrastructure { .. }
                | AppTask::RestoreOnInfrastructureAndDeleteFromBackup { .. }
        ) {
            return Err(InMemoryQueueError::BackUpAndRestoreIsNotSupported);
        }

        let mut queue = self.queue.lock().await;
        queue.push_back((task, AppTaskStatus::New));

        Ok(())
    }

    async fn peek_result(
        &self,
        status_id: &AppStatusChangeId,
    ) -> Option<std::result::Result<App, AppsError>> {
        let queue = self.queue.lock().await;

        for (task, status) in queue.iter() {
            if task.status_id() == status_id
                && let AppTaskStatus::Done((_, result)) = status
            {
                return Some(result.clone());
            }
        }
        None
    }

    async fn fetch_processable_tasks<'lc, 'a: 'lc>(
        &'a self,
    ) -> Result<Option<(super::ProcessableTasks, Self::LockContext<'lc>)>, Self::Error> {
        let lock = Uuid::new_v4();
        let mut queue = self.queue.lock().await;

        let tasks = {
            let mut iter = queue.iter_mut();

            let mut tasks = Vec::new();
            let mut app_name = None;
            for (task, s) in iter.by_ref() {
                app_name = match (&s, app_name.take()) {
                    (AppTaskStatus::New, current_app_name) => {
                        if current_app_name.is_some() && Some(task.app_name()) != current_app_name {
                            break;
                        }
                        *s = AppTaskStatus::InProcess(lock);
                        tasks.push(task.clone());
                        Some(task.app_name())
                    }
                    (AppTaskStatus::InProcess(_pre_existing_lock), None) => {
                        log::warn!(
                            "Trying to find tasks to be process but there is currently {} in process",
                            task.app_name()
                        );
                        tasks.clear();
                        break;
                    }
                    (AppTaskStatus::InProcess(_pre_existing_lock), Some(app_name)) => {
                        // TODO: elevate to error case…
                        log::error!(
                            "The interior queue status seem to be messed up while searching for tasks for {app_name} we found a in-process task for {}",
                            task.app_name()
                        );
                        tasks.clear();
                        break;
                    }
                    (AppTaskStatus::Done(_), _) => continue,
                }
            }
            tasks
        };

        if tasks.is_empty() {
            Ok(None)
        } else {
            let processable_tasks = match ProcessableTasks::new(tasks, Default::default()) {
                Ok(processable_tasks) => processable_tasks,
                Err(err) => {
                    let mut iter = queue.iter_mut();
                    for (_task, s) in iter.by_ref() {
                        match &s {
                            AppTaskStatus::InProcess(uuid) if *uuid == lock => {
                                *s = AppTaskStatus::New
                            }
                            _ => {}
                        }
                    }
                    return Err(InMemoryQueueError::FailedToFetchTasks(err));
                }
            };

            Ok(Some((processable_tasks, lock)))
        }
    }

    async fn store_result<'lc>(
        &self,
        lock_context: Self::LockContext<'lc>,
        result: Result<App, AppsError>,
        done_timestamp: DateTime<Utc>,
        merged: &MergedAppTask,
    ) -> Result<(), Self::Error> {
        let task_worked_on = &merged.task_to_work_on;
        let status_id = task_worked_on.status_id();

        let mut queue = self.queue.lock().await;
        let Some(task) = queue.iter_mut().find(|(task, lock_id)| {
            task.status_id() == status_id
                && matches!(lock_id, AppTaskStatus::InProcess(id) if *id == lock_context)
        }) else {
            return Err(InMemoryQueueError::CannotFindAppStatusChangeId {
                status_id: *status_id,
            });
        };

        task.1 = AppTaskStatus::Done((done_timestamp, result.clone()));

        for (task, s) in queue.iter_mut() {
            if merged.tasks_to_be_marked_as_done.contains(task.status_id()) {
                *s = AppTaskStatus::Done((done_timestamp, result.clone()));
            }
            if merged.tasks_to_stay_untouched.contains(task.status_id()) {
                *s = AppTaskStatus::New;
            }
        }

        Ok(())
    }

    async fn clean_up_done_tasks(&self, older_than: DateTime<Utc>) -> Result<usize, Self::Error> {
        let mut queue = self.queue.lock().await;

        let before = queue.len();
        queue.retain(
            |(_, status)| !matches!(status, AppTaskStatus::Done((timestamp, _)) if timestamp < &older_than),
        );

        Ok(before - queue.len())
    }
}
