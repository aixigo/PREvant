/*-
 * ========================LICENSE_START=================================
 * PREvant REST API
 * %%
 * Copyright (C) 2018 - 2019 aixigo AG
 * %%
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
 * THE SOFTWARE.
 * =========================LICENSE_END==================================
 */

use std::future::Future;
use std::task::Poll;
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::task::{JoinError, JoinHandle};
use tokio::time::timeout;

pub struct TasksService {
    runtime: Runtime,
}

pub enum RunOptions {
    Sync,
    Async { wait: Option<Duration> },
}

impl TasksService {
    pub fn new() -> Result<TasksService, TasksServiceError> {
        Ok(TasksService {
            runtime: Runtime::new()?,
        })
    }

    fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.runtime.handle().spawn(future)
    }

    async fn join<T: Send + 'static>(
        &self,
        options: RunOptions,
        handle: JoinHandle<T>,
    ) -> Result<Poll<T>, TasksServiceError> {
        match options {
            RunOptions::Sync => Ok(Poll::Ready(handle.await?)),
            RunOptions::Async { wait: None } => Ok(Poll::Pending),
            RunOptions::Async {
                wait: Some(duration),
            } => {
                match self.spawn(timeout(duration, handle)).await? {
                    // Execution completed before timeout
                    Ok(Ok(result)) => Ok(Poll::Ready(result)),
                    // JoinError occurred before timeout
                    Ok(Err(err)) => Err(TasksServiceError::from(err)),
                    // Timeout elapsed while waiting for future
                    Err(_) => Ok(Poll::Pending),
                }
            }
        }
    }

    pub async fn run<F>(
        &self,
        options: RunOptions,
        future: F,
    ) -> Result<Poll<F::Output>, TasksServiceError>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let handle = self.spawn(future);
        self.join(options, handle).await
    }

    pub async fn try_run<F>(
        &self,
        options: RunOptions,
        future: F,
    ) -> Result<Option<F::Output>, TasksServiceError>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        match options {
            RunOptions::Sync => Ok(Some(future.await)),
            RunOptions::Async { wait: duration } => {
                let duration = duration.unwrap_or(Duration::from_secs(0));
                match timeout(duration, future).await {
                    // Execution completed before timeout
                    Ok(result) => Ok(Some(result)),
                    // Timeout elapsed while waiting for future
                    Err(_) => Ok(None),
                }
            }
        }
    }
}

/// Defines error cases for the `TasksService`
#[derive(Debug, Clone, Fail)]
pub enum TasksServiceError {
    /// Will be used when there was an error with the `tokio::runtime::Runtime`
    #[fail(display = "Runtime error: {}.", msg)]
    RuntimeError { msg: String },
}

impl From<std::io::Error> for TasksServiceError {
    fn from(error: std::io::Error) -> Self {
        TasksServiceError::RuntimeError {
            msg: format!("{:?}", error),
        }
    }
}

impl From<JoinError> for TasksServiceError {
    fn from(error: JoinError) -> Self {
        TasksServiceError::RuntimeError {
            msg: format!("{:?}", error),
        }
    }
}
