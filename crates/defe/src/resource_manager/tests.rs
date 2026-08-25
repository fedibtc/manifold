use std::collections::HashMap;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;

use defe_api::ApiErrorKind;

use super::*;

#[test]
fn handles_are_scoped_to_a_connection_and_release_is_explicit() {
    let fake = FakeDriver::default();
    let manager = Arc::new(ResourceManager::new(fake.clone_driver()));
    let mut first = manager.connection();
    let mut second = manager.connection();

    let first_lease = first
        .allocate(fake_exclusive_spec())
        .expect("allocate first exclusive resource");
    let second_lease = second
        .allocate(fake_exclusive_spec())
        .expect("allocate second exclusive resource");

    assert_eq!(first_lease.handle_id, ResourceHandleId(1));
    assert_eq!(second_lease.handle_id, ResourceHandleId(1));
    assert_ne!(first_lease.descriptor, second_lease.descriptor);

    first
        .release(first_lease.handle_id)
        .expect("release first handle");
    assert_eq!(fake.stop_count(), 1);
    assert_eq!(fake.running_slot_count(), 1);

    let err = first
        .release(first_lease.handle_id)
        .expect_err("released handle is no longer owned");
    assert_eq!(err.kind, ApiErrorKind::UnknownHandle);

    second
        .release(second_lease.handle_id)
        .expect("second connection still owns its scoped handle");
    assert_eq!(fake.stop_count(), 2);
    assert_eq!(fake.running_slot_count(), 0);
}

#[test]
fn unknown_handle_on_another_connection_does_not_release_owner_resource() {
    let fake = FakeDriver::default();
    let manager = Arc::new(ResourceManager::new(fake.clone_driver()));
    let mut owner = manager.connection();
    let mut other = manager.connection();

    let lease = owner
        .allocate(fake_exclusive_spec())
        .expect("allocate exclusive resource");

    let err = other
        .release(lease.handle_id)
        .expect_err("other connection does not own the handle");
    assert_eq!(err.kind, ApiErrorKind::UnknownHandle);
    assert_eq!(fake.stop_count(), 0);

    owner.release(lease.handle_id).expect("owner can release");
    assert_eq!(fake.stop_count(), 1);
}

#[test]
fn dropping_connection_releases_remaining_exclusive_handles() {
    let fake = FakeDriver::default();
    let manager = Arc::new(ResourceManager::new(fake.clone_driver()));

    {
        let mut connection = manager.connection();
        connection
            .allocate(fake_exclusive_spec())
            .expect("allocate exclusive resource");
        assert_eq!(fake.running_slot_count(), 1);
    }

    assert_eq!(fake.stop_count(), 1);
    assert_eq!(fake.running_slot_count(), 0);
}

#[test]
fn shared_resources_are_reused_until_last_lease_then_recreated() {
    let fake = FakeDriver::default();
    let manager = Arc::new(ResourceManager::new(fake.clone_driver()));
    let mut first = manager.connection();
    let mut second = manager.connection();

    let first_lease = first
        .allocate(fake_shared_spec("relay"))
        .expect("allocate shared resource");
    let second_lease = second
        .allocate(fake_shared_spec("relay"))
        .expect("reuse shared resource");

    assert_eq!(first_lease.descriptor, second_lease.descriptor);
    assert_eq!(fake.start_count(), 1);
    assert_eq!(fake.running_slot_count(), 1);

    first.release(first_lease.handle_id).expect("release first");
    assert_eq!(fake.stop_count(), 0, "one shared lease is still alive");
    assert_eq!(fake.running_slot_count(), 1);

    second
        .release(second_lease.handle_id)
        .expect("release second");
    assert_eq!(fake.stop_count(), 1, "last shared lease stops the slot");
    assert_eq!(fake.running_slot_count(), 0);

    let mut third = manager.connection();
    let third_lease = third
        .allocate(fake_shared_spec("relay"))
        .expect("start a fresh shared resource");
    assert_ne!(third_lease.descriptor, first_lease.descriptor);
    assert_eq!(fake.start_count(), 2);
    assert_eq!(fake.running_slot_count(), 1);

    third.release(third_lease.handle_id).expect("release third");
    assert_eq!(fake.stop_count(), 2);
    assert_eq!(fake.running_slot_count(), 0);
}

#[test]
fn dropping_connections_releases_last_shared_lease() {
    let fake = FakeDriver::default();
    let manager = Arc::new(ResourceManager::new(fake.clone_driver()));
    let mut first = manager.connection();
    let mut second = manager.connection();

    first
        .allocate(fake_shared_spec("relay"))
        .expect("allocate shared resource");
    second
        .allocate(fake_shared_spec("relay"))
        .expect("reuse shared resource");
    assert_eq!(fake.start_count(), 1);
    assert_eq!(fake.running_slot_count(), 1);

    drop(first);
    assert_eq!(fake.stop_count(), 0, "second shared lease is still alive");
    assert_eq!(fake.running_slot_count(), 1);

    drop(second);
    assert_eq!(fake.stop_count(), 1, "last shared lease stops the slot");
    assert_eq!(fake.running_slot_count(), 0);
}

#[test]
fn concurrent_shared_requests_start_only_one_resource() {
    let fake = FakeDriver::default();
    let manager = Arc::new(ResourceManager::new(fake.clone_driver()));
    let start_barrier = Arc::new(Barrier::new(8));
    let release_barrier = Arc::new(Barrier::new(8));
    let mut threads = Vec::new();

    for _ in 0..8 {
        let manager = Arc::clone(&manager);
        let start_barrier = Arc::clone(&start_barrier);
        let release_barrier = Arc::clone(&release_barrier);
        threads.push(thread::spawn(move || {
            let mut connection = manager.connection();
            start_barrier.wait();
            let descriptor = connection
                .allocate(fake_shared_spec("concurrent"))
                .expect("allocate shared resource")
                .descriptor;
            release_barrier.wait();
            descriptor
        }));
    }

    let descriptors = threads
        .into_iter()
        .map(|thread| thread.join().expect("thread succeeds"))
        .collect::<Vec<_>>();
    let first_descriptor = descriptors.first().expect("threads returned descriptors");
    assert!(
        descriptors
            .iter()
            .all(|descriptor| descriptor == first_descriptor)
    );
    assert_eq!(fake.start_count(), 1);
    assert_eq!(fake.stop_count(), 1);
    assert_eq!(fake.running_slot_count(), 0);

    manager.shutdown();
    assert_eq!(fake.stop_count(), 1);
}

#[test]
fn shutdown_rejects_new_allocations() {
    let fake = FakeDriver::default();
    let manager = Arc::new(ResourceManager::new(fake.clone_driver()));

    manager.shutdown();

    let mut connection = manager.connection();
    let err = connection
        .allocate(fake_exclusive_spec())
        .expect_err("closed manager rejects allocation");
    assert_eq!(err.kind, ApiErrorKind::InternalServerError);
    assert_eq!(fake.start_count(), 0);
}

#[test]
fn shutdown_rejects_restart_of_existing_handles() {
    let fake = FakeDriver::default();
    let manager = Arc::new(ResourceManager::new(fake.clone_driver()));
    let mut connection = manager.connection();
    let lease = connection
        .allocate(fake_exclusive_spec())
        .expect("allocate exclusive resource");

    manager.shutdown();

    let err = connection
        .restart(lease.handle_id, RestartMode::Force)
        .expect_err("closed manager rejects restart");
    assert_eq!(err.kind, ApiErrorKind::InternalServerError);
    assert_eq!(fake.start_count(), 1);
    assert_eq!(fake.stop_count(), 1);
}

#[test]
fn exclusive_resources_do_not_share_slots() {
    let fake = FakeDriver::default();
    let manager = Arc::new(ResourceManager::new(fake.clone_driver()));
    let mut connection = manager.connection();

    let first = connection
        .allocate(fake_exclusive_spec())
        .expect("allocate first exclusive resource");
    let second = connection
        .allocate(fake_exclusive_spec())
        .expect("allocate second exclusive resource");

    assert_ne!(first.descriptor, second.descriptor);
    assert_eq!(fake.start_count(), 2);

    drop(connection);
    assert_eq!(fake.stop_count(), 2);
}

#[test]
fn restart_if_exited_refuses_a_running_resource_then_restarts_exited_one() {
    let fake = FakeDriver::default();
    let manager = Arc::new(ResourceManager::new(fake.clone_driver()));
    let mut connection = manager.connection();
    let lease = connection
        .allocate(fake_exclusive_spec())
        .expect("allocate exclusive resource");
    let slot_id = slot_id_from_descriptor(&lease.descriptor);

    let err = connection
        .restart(lease.handle_id, RestartMode::IfExited)
        .expect_err("running resource is not restarted by IfExited");
    assert_eq!(err.kind, ApiErrorKind::ResourceRestartRefused);
    assert_eq!(fake.start_count(), 1);

    fake.mark_exited(slot_id);
    let restarted = connection
        .restart(lease.handle_id, RestartMode::IfExited)
        .expect("restart exited resource");

    assert_eq!(restarted.handle_id, lease.handle_id);
    assert_ne!(restarted.descriptor, lease.descriptor);
    assert_eq!(fake.start_count(), 2);
    assert_eq!(fake.stop_count(), 0);
    assert!(fake.is_running(slot_id));
}

#[test]
fn restart_force_stops_running_resource_and_preserves_the_slot() {
    let fake = FakeDriver::default();
    let manager = Arc::new(ResourceManager::new(fake.clone_driver()));
    let mut connection = manager.connection();
    let lease = connection
        .allocate(fake_exclusive_spec())
        .expect("allocate exclusive resource");
    let slot_id = slot_id_from_descriptor(&lease.descriptor);

    let restarted = connection
        .restart(lease.handle_id, RestartMode::Force)
        .expect("force restart running resource");

    assert_eq!(restarted.handle_id, lease.handle_id);
    assert_eq!(slot_id_from_descriptor(&restarted.descriptor), slot_id);
    assert_ne!(restarted.descriptor, lease.descriptor);
    assert_eq!(fake.start_count(), 2);
    assert_eq!(fake.stop_count(), 1);
    assert!(fake.is_running(slot_id));
}

fn fake_exclusive_spec() -> ResourceSpec {
    ResourceSpec::exclusive(ResourceKind::Fake)
}

fn fake_shared_spec(name: &str) -> ResourceSpec {
    ResourceSpec::shared(ResourceKind::Fake, SharedResourceKey::Fake(name.to_owned()))
}

fn slot_id_from_descriptor(descriptor: &ResourceDescriptor) -> ResourceSlotId {
    let ResourceDescriptor::NostrRelay(info) = descriptor else {
        panic!("expected Nostr relay descriptor, got {descriptor:?}");
    };
    let slot = info
        .url
        .strip_prefix("fake://slot-")
        .and_then(|rest| rest.split_once('/'))
        .map(|(slot, _generation)| slot)
        .expect("fake descriptor url includes slot id");
    ResourceSlotId(slot.parse().expect("slot id is numeric"))
}

#[derive(Clone, Default)]
struct FakeDriver {
    inner: Arc<Mutex<FakeDriverState>>,
}

impl FakeDriver {
    fn clone_driver(&self) -> Arc<dyn ResourceDriver> {
        Arc::new(self.clone())
    }

    fn start_count(&self) -> usize {
        self.inner.lock().expect("fake mutex").starts
    }

    fn stop_count(&self) -> usize {
        self.inner.lock().expect("fake mutex").stops
    }

    fn running_slot_count(&self) -> usize {
        self.inner
            .lock()
            .expect("fake mutex")
            .running
            .values()
            .filter(|resource| resource.running)
            .count()
    }

    fn mark_exited(&self, slot_id: ResourceSlotId) {
        let mut inner = self.inner.lock().expect("fake mutex");
        let resource = inner
            .running
            .get_mut(&slot_id)
            .expect("slot exists in fake driver");
        resource.running = false;
    }

    fn is_running(&self, slot_id: ResourceSlotId) -> bool {
        self.inner
            .lock()
            .expect("fake mutex")
            .running
            .get(&slot_id)
            .is_some_and(|resource| resource.running)
    }
}

impl ResourceDriver for FakeDriver {
    fn start(&self, allocation: &ResourceAllocation) -> Result<Box<dyn ManagedResource>, ApiError> {
        let descriptor = fake_nostr_descriptor(allocation.slot_id, allocation.generation);
        let mut inner = self.inner.lock().expect("fake mutex");
        inner.starts += 1;
        inner
            .running
            .insert(allocation.slot_id, FakeResourceState { running: true });
        Ok(Box::new(FakeResource {
            driver: self.clone(),
            slot_id: allocation.slot_id,
            descriptor,
        }))
    }
}

#[derive(Default)]
struct FakeDriverState {
    starts: usize,
    stops: usize,
    running: HashMap<ResourceSlotId, FakeResourceState>,
}

struct FakeResourceState {
    running: bool,
}

struct FakeResource {
    driver: FakeDriver,
    slot_id: ResourceSlotId,
    descriptor: ResourceDescriptor,
}

impl ManagedResource for FakeResource {
    fn descriptor(&self) -> ResourceDescriptor {
        self.descriptor.clone()
    }

    fn is_running(&self) -> bool {
        self.driver.is_running(self.slot_id)
    }

    fn stop(&mut self) {
        let mut inner = self.driver.inner.lock().expect("fake mutex");
        let Some(resource) = inner.running.get_mut(&self.slot_id) else {
            return;
        };
        if resource.running {
            resource.running = false;
            inner.stops += 1;
        }
    }
}
