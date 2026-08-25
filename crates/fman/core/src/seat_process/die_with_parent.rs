//! Creates Linux seat children from one process-lifetime OS thread.
//!
//! Linux delivers `PR_SET_PDEATHSIG` when the specific thread that created a
//! child exits, not only when its process exits. Tokio worker threads may
//! retire during ordinary runtime operation, so every child must be created
//! here rather than by whichever worker happens to poll the seat process.

use std::sync::{OnceLock, mpsc};

use tokio::process::{Child, Command};
use tokio::sync::oneshot;

struct ChildSpawner {
    requests: mpsc::Sender<ChildSpawnRequest>,
}

struct ChildSpawnRequest {
    command: Command,
    runtime: tokio::runtime::Handle,
    reply: oneshot::Sender<std::io::Result<Child>>,
}

static CHILD_SPAWNER: OnceLock<ChildSpawner> = OnceLock::new();

pub(super) async fn spawn(command: Command) -> std::io::Result<Child> {
    let (reply, response) = oneshot::channel();
    ChildSpawner::global()?
        .requests
        .send(ChildSpawnRequest {
            command,
            runtime: tokio::runtime::Handle::current(),
            reply,
        })
        .map_err(|_| stopped_error())?;
    response.await.map_err(|_| stopped_error())?
}

impl ChildSpawner {
    fn global() -> std::io::Result<&'static Self> {
        if let Some(spawner) = CHILD_SPAWNER.get() {
            return Ok(spawner);
        }
        let spawner = Self::start()?;
        Ok(CHILD_SPAWNER.get_or_init(|| spawner))
    }

    fn start() -> std::io::Result<Self> {
        let (requests, receiver) = mpsc::channel::<ChildSpawnRequest>();
        std::thread::Builder::new()
            .name("fman-child-spawner".to_owned())
            .spawn(move || {
                while let Ok(request) = receiver.recv() {
                    let ChildSpawnRequest {
                        mut command,
                        runtime,
                        reply,
                    } = request;
                    let _runtime = runtime.enter();
                    let spawn_result = command.spawn();
                    let _ = reply.send(spawn_result);
                }
            })?;
        Ok(Self { requests })
    }
}

fn stopped_error() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "fedimintd child-spawner thread stopped",
    )
}
