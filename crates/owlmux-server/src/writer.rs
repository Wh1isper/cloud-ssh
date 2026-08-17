use std::{collections::HashMap, sync::Arc};

use tokio::sync::{Mutex, MutexGuard, mpsc, oneshot, watch};
use uuid::Uuid;

use crate::relay::RouteIdentity;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClientSize {
    pub columns: u32,
    pub rows: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlOutcome {
    Succeeded,
    Failed,
    Ambiguous,
}

pub enum ControlDirective {
    Demote {
        response: oneshot::Sender<ControlOutcome>,
    },
    Close,
}

#[derive(Clone)]
struct WriterHandle {
    attachment_id: Uuid,
    size: ClientSize,
    control: Option<mpsc::Sender<ControlDirective>>,
}

#[derive(Default)]
struct WriterState {
    current: Option<WriterHandle>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WriterView {
    pub is_writer: bool,
    pub writer_available: bool,
}

pub struct WriterScope {
    route: RouteIdentity,
    dispatch: Arc<Mutex<()>>,
    state: Mutex<WriterState>,
    changed: watch::Sender<u64>,
}

impl WriterScope {
    fn new(route: RouteIdentity) -> Arc<Self> {
        let (changed, _) = watch::channel(0);
        Arc::new(Self {
            route,
            dispatch: Arc::new(Mutex::new(())),
            state: Mutex::new(WriterState::default()),
            changed,
        })
    }

    #[must_use]
    pub const fn route(&self) -> RouteIdentity {
        self.route
    }

    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    pub async fn view(&self, attachment_id: Uuid) -> WriterView {
        let state = self.state.lock().await;
        WriterView {
            is_writer: state
                .current
                .as_ref()
                .is_some_and(|writer| writer.attachment_id == attachment_id),
            writer_available: state.current.is_none(),
        }
    }

    pub async fn dispatch(&self) -> MutexGuard<'_, ()> {
        self.dispatch.lock().await
    }

    pub fn try_dispatch(&self) -> Option<MutexGuard<'_, ()>> {
        self.dispatch.try_lock().ok()
    }

    pub async fn dispatch_owned(self: Arc<Self>) -> tokio::sync::OwnedMutexGuard<()> {
        Arc::clone(&self.dispatch).lock_owned().await
    }

    pub async fn lock(&self) -> WriterGuard<'_> {
        WriterGuard {
            state: self.state.lock().await,
            changed: &self.changed,
        }
    }

    async fn release(&self, attachment_id: Uuid) {
        let _dispatch = self.dispatch().await;
        let mut guard = self.lock().await;
        guard.clear_if_current(attachment_id);
    }
}

pub struct WriterGuard<'a> {
    state: MutexGuard<'a, WriterState>,
    changed: &'a watch::Sender<u64>,
}

impl WriterGuard<'_> {
    #[must_use]
    pub fn is_current(&self, attachment_id: Uuid) -> bool {
        self.state
            .current
            .as_ref()
            .is_some_and(|writer| writer.attachment_id == attachment_id)
    }

    #[must_use]
    pub fn is_available(&self) -> bool {
        self.state.current.is_none()
    }

    #[must_use]
    pub fn current_attachment(&self) -> Option<Uuid> {
        self.state
            .current
            .as_ref()
            .map(|writer| writer.attachment_id)
    }

    #[must_use]
    pub fn current_control(&self) -> Option<mpsc::Sender<ControlDirective>> {
        self.state
            .current
            .as_ref()
            .and_then(|writer| writer.control.clone())
    }

    #[must_use]
    pub fn current_size(&self, attachment_id: Uuid) -> Option<ClientSize> {
        self.state
            .current
            .as_ref()
            .and_then(|writer| (writer.attachment_id == attachment_id).then_some(writer.size))
    }

    pub fn set_current(
        &mut self,
        attachment_id: Uuid,
        size: ClientSize,
        control: Option<mpsc::Sender<ControlDirective>>,
    ) {
        self.state.current = Some(WriterHandle {
            attachment_id,
            size,
            control,
        });
        self.notify();
    }

    pub fn set_control(
        &mut self,
        attachment_id: Uuid,
        control: mpsc::Sender<ControlDirective>,
    ) -> bool {
        let Some(writer) = self.state.current.as_mut() else {
            return false;
        };
        if writer.attachment_id != attachment_id {
            return false;
        }
        writer.control = Some(control);
        true
    }

    pub fn clear_control(&mut self, attachment_id: Uuid) -> bool {
        let Some(writer) = self.state.current.as_mut() else {
            return false;
        };
        if writer.attachment_id != attachment_id {
            return false;
        }
        writer.control = None;
        true
    }

    pub fn update_size(&mut self, attachment_id: Uuid, size: ClientSize) -> bool {
        let Some(writer) = self.state.current.as_mut() else {
            return false;
        };
        if writer.attachment_id != attachment_id {
            return false;
        }
        writer.size = size;
        true
    }

    pub fn clear_if_current(&mut self, attachment_id: Uuid) -> bool {
        if !self.is_current(attachment_id) {
            return false;
        }
        self.state.current = None;
        self.notify();
        true
    }

    pub fn clear(&mut self) {
        if self.state.current.take().is_some() {
            self.notify();
        }
    }

    fn notify(&self) {
        self.changed
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
}

#[derive(Clone, Default)]
pub struct WriterRegistry {
    scopes: Arc<Mutex<HashMap<Uuid, Arc<WriterScope>>>>,
}

impl WriterRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn scope(&self, machine_id: Uuid, route: RouteIdentity) -> Arc<WriterScope> {
        let mut scopes = self.scopes.lock().await;
        if let Some(scope) = scopes.get(&machine_id)
            && scope.route == route
        {
            return Arc::clone(scope);
        }
        let scope = WriterScope::new(route);
        scopes.insert(machine_id, Arc::clone(&scope));
        scope
    }

    pub async fn release_attachment(&self, attachment_id: Uuid) {
        let scopes = self
            .scopes
            .lock()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for scope in scopes {
            scope.release(attachment_id).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(connection_epoch: i64) -> RouteIdentity {
        RouteIdentity {
            route_revision: 1,
            connection_epoch,
            connection_id: Uuid::new_v4(),
        }
    }

    #[tokio::test]
    async fn writer_pointer_is_single_and_route_scoped() {
        let registry = WriterRegistry::new();
        let machine_id = Uuid::new_v4();
        let first = registry.scope(machine_id, route(1)).await;
        let first_attachment = Uuid::new_v4();
        let second_attachment = Uuid::new_v4();
        {
            let mut guard = first.lock().await;
            assert!(guard.is_available());
            guard.set_current(
                first_attachment,
                ClientSize {
                    columns: 80,
                    rows: 24,
                },
                None,
            );
            assert!(guard.is_current(first_attachment));
            assert!(!guard.is_current(second_attachment));
        }
        registry.release_attachment(second_attachment).await;
        assert!(first.view(first_attachment).await.is_writer);
        registry.release_attachment(first_attachment).await;
        assert!(first.view(second_attachment).await.writer_available);

        let replacement_route = route(2);
        let replacement = registry.scope(machine_id, replacement_route).await;
        assert_eq!(replacement.route(), replacement_route);
        assert!(replacement.view(second_attachment).await.writer_available);
    }
}
