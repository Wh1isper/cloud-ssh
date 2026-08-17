use std::{future::Future, time::Duration};

use tokio::time::timeout;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

#[derive(Clone)]
pub struct RuntimeTasks {
    cancellation: CancellationToken,
    tracker: TaskTracker,
}

impl RuntimeTasks {
    #[must_use]
    pub fn new() -> Self {
        Self {
            cancellation: CancellationToken::new(),
            tracker: TaskTracker::new(),
        }
    }

    #[must_use]
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    pub fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) {
        self.tracker.spawn(future);
    }

    pub async fn shutdown(self, deadline: Duration) -> bool {
        self.cancellation.cancel();
        self.tracker.close();
        timeout(deadline, self.tracker.wait()).await.is_ok()
    }
}

impl Default for RuntimeTasks {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use super::*;

    #[tokio::test]
    async fn cancellation_reaches_owned_tasks() {
        let tasks = RuntimeTasks::new();
        let token = tasks.cancellation_token();
        let stopped = Arc::new(AtomicBool::new(false));
        let task_stopped = Arc::clone(&stopped);
        tasks.spawn(async move {
            token.cancelled().await;
            task_stopped.store(true, Ordering::SeqCst);
        });

        assert!(tasks.shutdown(Duration::from_secs(1)).await);
        assert!(stopped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn shutdown_is_bounded() {
        let tasks = RuntimeTasks::new();
        tasks.spawn(std::future::pending());
        assert!(!tasks.shutdown(Duration::from_millis(1)).await);
    }
}
