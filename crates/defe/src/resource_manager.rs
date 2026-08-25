use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use defe_api::{
    ApiError, ApiErrorKind, NostrRelayInfo, ResourceDescriptor, ResourceHandleId, ResourceLease,
    RestartMode,
};

/// Stable id for a logical resource slot managed by [`ResourceManager`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResourceSlotId(
    /// Numeric slot identifier assigned by the resource manager.
    pub u64,
);

/// Kinds of resources known to the server-side resource manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceKind {
    /// A local `nostr-rs-relay` process with a WebSocket endpoint.
    NostrRelay,
    /// A local push gateway HTTP server process.
    PushGateway,
    /// A local Bitcoin Core regtest node.
    Bitcoind,
    /// A local Fleet Manager process configured for one future federation seat.
    Fman(defe_api::FmanRequest),
    /// A local FLIP daemon process.
    Flip(defe_api::FlipRequest),
    /// A local Fedimint gateway daemon process.
    Gatewayd(defe_api::GatewaydRequest),
    /// In-memory fake resource used by resource-manager tests.
    Fake,
}

/// Key used to find compatible shared resources.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SharedResourceKey {
    /// Shared key for the default Nostr relay resource.
    NostrRelay,
    /// Shared key for the default push gateway resource.
    PushGateway,
    /// Shared key for the default Bitcoin Core regtest resource.
    Bitcoind,
    /// Shared key for a Fleet Manager with matching launch topology.
    Fman(defe_api::FmanRequest),
    /// Shared key for a FLIP daemon with matching launch inputs.
    Flip(defe_api::FlipRequest),
    /// Shared key for a gateway with matching launch inputs.
    Gatewayd(defe_api::GatewaydRequest),
    /// Named fake key used by resource-manager tests.
    Fake(String),
}

/// Whether a resource allocation should reuse a shared slot or create a private slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceSharing {
    /// Reuse a resource slot associated with the supplied key, if one exists.
    Shared(SharedResourceKey),
    /// Create a new resource slot for this allocation only.
    Exclusive,
}

/// Internal allocation request handled by the resource manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceSpec {
    /// Concrete resource kind that should be started if a new slot is needed.
    pub kind: ResourceKind,
    /// Sharing policy used to choose between shared and private slots.
    pub sharing: ResourceSharing,
}

impl ResourceSpec {
    /// Build a resource specification for a private resource slot.
    #[must_use]
    pub const fn exclusive(kind: ResourceKind) -> Self {
        Self {
            kind,
            sharing: ResourceSharing::Exclusive,
        }
    }

    /// Build a resource specification for a shared resource slot.
    #[must_use]
    pub const fn shared(kind: ResourceKind, key: SharedResourceKey) -> Self {
        Self {
            kind,
            sharing: ResourceSharing::Shared(key),
        }
    }
}

/// Stable allocation data passed to a concrete resource implementation when it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceAllocation {
    /// Slot identifier that remains stable across restarts of the same slot.
    pub slot_id: ResourceSlotId,
    /// Resource kind the driver is expected to start.
    pub kind: ResourceKind,
    /// Shared key associated with the slot, or `None` for exclusive slots.
    pub sharing_key: Option<SharedResourceKey>,
    /// Monotonic generation number incremented every time the slot is restarted.
    pub generation: u64,
}

/// Concrete running or restartable resource instance owned by a resource slot.
pub trait ManagedResource: Send {
    /// Return client-visible connection information for the resource.
    fn descriptor(&self) -> ResourceDescriptor;

    /// Report whether the underlying resource process is currently running.
    fn is_running(&self) -> bool;

    /// Stop the underlying resource and release any external process state.
    fn stop(&mut self);
}

/// Starts concrete resources for slots owned by [`ResourceManager`].
pub trait ResourceDriver: Send + Sync {
    /// Start the resource described by `allocation` and return its managed instance.
    fn start(&self, allocation: &ResourceAllocation) -> Result<Box<dyn ManagedResource>, ApiError>;
}

/// Owns resource slots shared by all client connections.
///
/// TODO: start, restart, and stop operations currently run while the manager
/// mutex is held. Releasing it safely requires an explicit per-slot
/// "starting/stopping" state so concurrent shared allocations still coalesce to
/// a single process start and handles cannot observe a half-restarted slot.
pub struct ResourceManager {
    /// Mutable slot registry protected by a mutex for cross-client access.
    inner: Mutex<ManagerInner>,
    /// Driver used to start concrete resource implementations.
    driver: Arc<dyn ResourceDriver>,
}

impl ResourceManager {
    /// Create an empty resource manager backed by the supplied resource driver.
    #[must_use]
    pub fn new(driver: Arc<dyn ResourceDriver>) -> Self {
        Self {
            inner: Mutex::new(ManagerInner::default()),
            driver,
        }
    }

    /// Create a new client-scoped connection handle into this manager.
    #[must_use]
    pub fn connection(self: &Arc<Self>) -> ResourceConnection {
        ResourceConnection {
            manager: Arc::clone(self),
            handles: HashMap::new(),
            next_handle_id: 1,
        }
    }

    /// Stop all managed resources and reject future allocation or restart requests.
    pub fn shutdown(&self) {
        let mut inner = self.lock_inner();
        inner.closed = true;
        inner.shared.clear();
        inner.stop_all_slots();
    }

    fn acquire(
        &self,
        spec: &ResourceSpec,
    ) -> Result<(ResourceSlotId, ResourceDescriptor), ApiError> {
        match &spec.sharing {
            ResourceSharing::Shared(key) => self.acquire_shared(spec, key.clone()),
            ResourceSharing::Exclusive => self.acquire_exclusive(spec),
        }
    }

    fn acquire_shared(
        &self,
        spec: &ResourceSpec,
        key: SharedResourceKey,
    ) -> Result<(ResourceSlotId, ResourceDescriptor), ApiError> {
        let mut inner = self.lock_inner();
        if inner.closed {
            return Err(manager_closed_error());
        }
        if let Some(slot_id) = inner.shared.get(&key).copied() {
            let slot = inner.slots.get_mut(&slot_id).ok_or_else(|| {
                ApiError::new(
                    ApiErrorKind::InternalServerError,
                    "shared resource map points at a missing slot",
                )
            })?;
            slot.lease_count += 1;
            return Ok((slot_id, slot.latest_descriptor.clone()));
        }

        let slot_id = inner.next_slot_id();
        let allocation = ResourceAllocation {
            slot_id,
            kind: spec.kind.clone(),
            sharing_key: Some(key.clone()),
            generation: 1,
        };
        let resource = self.driver.start(&allocation)?;
        let descriptor = resource.descriptor();
        let slot = ResourceSlot {
            id: slot_id,
            kind: spec.kind.clone(),
            sharing_key: Some(key.clone()),
            generation: 1,
            lease_count: 1,
            resource,
            latest_descriptor: descriptor.clone(),
        };
        inner.shared.insert(key, slot_id);
        inner.slots.insert(slot_id, slot);
        Ok((slot_id, descriptor))
    }

    fn acquire_exclusive(
        &self,
        spec: &ResourceSpec,
    ) -> Result<(ResourceSlotId, ResourceDescriptor), ApiError> {
        let mut inner = self.lock_inner();
        if inner.closed {
            return Err(manager_closed_error());
        }
        let slot_id = inner.next_slot_id();
        let allocation = ResourceAllocation {
            slot_id,
            kind: spec.kind.clone(),
            sharing_key: None,
            generation: 1,
        };
        let resource = self.driver.start(&allocation)?;
        let descriptor = resource.descriptor();
        let slot = ResourceSlot {
            id: slot_id,
            kind: spec.kind.clone(),
            sharing_key: None,
            generation: 1,
            lease_count: 1,
            resource,
            latest_descriptor: descriptor.clone(),
        };
        inner.slots.insert(slot_id, slot);
        Ok((slot_id, descriptor))
    }

    fn release_slot(&self, slot_id: ResourceSlotId) {
        let mut inner = self.lock_inner();
        let sharing_key = {
            let Some(slot) = inner.slots.get_mut(&slot_id) else {
                return;
            };

            slot.lease_count = slot.lease_count.saturating_sub(1);
            if slot.lease_count > 0 {
                return;
            }
            slot.sharing_key.clone()
        };

        if let Some(key) = sharing_key
            && inner.shared.get(&key) == Some(&slot_id)
        {
            inner.shared.remove(&key);
        }

        if let Some(mut slot) = inner.slots.remove(&slot_id) {
            slot.resource.stop();
        }
    }

    fn restart_slot(
        &self,
        slot_id: ResourceSlotId,
        mode: RestartMode,
    ) -> Result<ResourceDescriptor, ApiError> {
        let mut inner = self.lock_inner();
        if inner.closed {
            return Err(manager_closed_error());
        }
        let slot = inner.slots.get_mut(&slot_id).ok_or_else(|| {
            ApiError::new(
                ApiErrorKind::UnknownHandle,
                "resource handle points at a missing slot",
            )
        })?;

        if mode == RestartMode::IfExited && slot.resource.is_running() {
            return Err(ApiError::new(
                ApiErrorKind::ResourceRestartRefused,
                "resource is still running",
            ));
        }

        if mode == RestartMode::Force {
            slot.resource.stop();
        }

        let next_generation = slot.generation + 1;
        let allocation = ResourceAllocation {
            slot_id: slot.id,
            kind: slot.kind.clone(),
            sharing_key: slot.sharing_key.clone(),
            generation: next_generation,
        };
        let resource = self.driver.start(&allocation)?;
        let descriptor = resource.descriptor();
        slot.resource = resource;
        slot.latest_descriptor = descriptor.clone();
        slot.generation = next_generation;
        Ok(descriptor)
    }

    fn lock_inner(&self) -> MutexGuard<'_, ManagerInner> {
        self.inner.lock().expect("resource manager mutex poisoned")
    }
}
fn manager_closed_error() -> ApiError {
    ApiError::new(
        ApiErrorKind::InternalServerError,
        "resource manager is shut down",
    )
}

impl Drop for ResourceManager {
    fn drop(&mut self) {
        let Ok(inner) = self.inner.get_mut() else {
            return;
        };
        inner.shared.clear();
        inner.stop_all_slots();
    }
}

/// Resource handles owned by one client connection.
pub struct ResourceConnection {
    /// Shared manager that owns the actual resource slots.
    manager: Arc<ResourceManager>,
    /// Mapping from connection-local handles to manager-wide resource slots.
    handles: HashMap<ResourceHandleId, ResourceSlotId>,
    /// Next connection-local handle id to assign.
    next_handle_id: u64,
}

impl ResourceConnection {
    /// Allocate a resource for this connection and return a client-visible lease.
    pub fn allocate(&mut self, spec: ResourceSpec) -> Result<ResourceLease, ApiError> {
        let (slot_id, descriptor) = self.manager.acquire(&spec)?;
        let handle_id = self.next_handle_id();
        self.handles.insert(handle_id, slot_id);
        Ok(ResourceLease {
            handle_id,
            descriptor,
        })
    }

    /// Release a handle owned by this connection.
    pub fn release(&mut self, handle_id: ResourceHandleId) -> Result<(), ApiError> {
        let slot_id = self.handles.remove(&handle_id).ok_or_else(|| {
            ApiError::new(
                ApiErrorKind::UnknownHandle,
                "resource handle is not owned by this connection",
            )
        })?;
        self.manager.release_slot(slot_id);
        Ok(())
    }

    /// Restart a resource handle owned by this connection.
    pub fn restart(
        &mut self,
        handle_id: ResourceHandleId,
        mode: RestartMode,
    ) -> Result<ResourceLease, ApiError> {
        let slot_id = self.handles.get(&handle_id).copied().ok_or_else(|| {
            ApiError::new(
                ApiErrorKind::UnknownHandle,
                "resource handle is not owned by this connection",
            )
        })?;
        let descriptor = self.manager.restart_slot(slot_id, mode)?;
        Ok(ResourceLease {
            handle_id,
            descriptor,
        })
    }

    /// Return the number of live handles owned by this connection.
    #[must_use]
    pub fn handle_count(&self) -> usize {
        self.handles.len()
    }

    fn next_handle_id(&mut self) -> ResourceHandleId {
        let handle_id = ResourceHandleId(self.next_handle_id);
        self.next_handle_id += 1;
        handle_id
    }
}

impl Drop for ResourceConnection {
    fn drop(&mut self) {
        for slot_id in self.handles.drain().map(|(_handle_id, slot_id)| slot_id) {
            self.manager.release_slot(slot_id);
        }
    }
}

#[derive(Default)]
struct ManagerInner {
    /// Next manager-wide slot id to assign.
    next_slot_id: u64,
    /// All currently allocated slots indexed by slot id.
    slots: HashMap<ResourceSlotId, ResourceSlot>,
    /// Whether the manager has been shut down.
    closed: bool,
    /// Shared-resource lookup from compatibility key to live slot id.
    shared: HashMap<SharedResourceKey, ResourceSlotId>,
}

impl ManagerInner {
    fn next_slot_id(&mut self) -> ResourceSlotId {
        self.next_slot_id += 1;
        ResourceSlotId(self.next_slot_id)
    }

    fn stop_all_slots(&mut self) {
        for slot in self.slots.values_mut() {
            slot.resource.stop();
        }
        self.slots.clear();
    }
}

struct ResourceSlot {
    /// Stable id of this slot.
    id: ResourceSlotId,
    /// Resource kind running in the slot.
    kind: ResourceKind,
    /// Shared lookup key, or `None` for an exclusive slot.
    sharing_key: Option<SharedResourceKey>,
    /// Current resource generation, incremented on restart.
    generation: u64,
    /// Number of connection-local handles currently leasing this slot.
    lease_count: usize,
    /// Managed resource instance for the current generation.
    resource: Box<dyn ManagedResource>,
    /// Most recent descriptor returned by the resource driver.
    latest_descriptor: ResourceDescriptor,
}

/// Driver that can be used until real resource implementations are wired in.
pub struct UnavailableResourceDriver;

impl ResourceDriver for UnavailableResourceDriver {
    /// Always reject resource starts because no concrete implementation is available.
    fn start(&self, allocation: &ResourceAllocation) -> Result<Box<dyn ManagedResource>, ApiError> {
        Err(ApiError::new(
            ApiErrorKind::ResourceKindUnavailable,
            format!("resource kind {:?} is not available", allocation.kind),
        ))
    }
}

/// Builds a deterministic descriptor suitable for fake-resource tests.
#[must_use]
pub fn fake_nostr_descriptor(slot_id: ResourceSlotId, generation: u64) -> ResourceDescriptor {
    ResourceDescriptor::NostrRelay(NostrRelayInfo {
        url: format!("fake://slot-{}/generation-{generation}", slot_id.0),
        host: "fake.local".to_owned(),
        port: u16::try_from(generation).unwrap_or(u16::MAX),
        data_dir: PathBuf::from(format!("/fake/slot-{}/generation-{generation}", slot_id.0)),
    })
}

#[cfg(test)]
mod tests;
