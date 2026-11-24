//! The host's coordinator for its seats' AlephBFT backup syncs.
//!
//! Every seat's fedimintd syncs its AlephBFT backup once per round. On btrfs,
//! which StartOS gives every data volume, each sync writes a fixed amount of
//! metadata however little it stores, but syncs that run at the same moment
//! share one filesystem commit. The seats' rounds already end on a shared
//! grid (fedimintd aligns them to the host's phase when it is given this
//! socket and phase); the coordinator closes the remaining gap by releasing
//! the seats' commits together.
//!
//! The phase is random per daemon start. All seats of a host share it, so
//! their units, and syncs, line up. The peers of one federation sit on
//! different hosts, so they keep different phases: a whole federation in
//! lockstep orders its items several rounds later.
//!
//! Protocol, per connection: the seat sends `r` when its backup commit is
//! ready and commits when it reads `g`. The first `r` opens a window. The
//! coordinator answers every waiting seat when all active seats are waiting,
//! or when the window ends. A seat is active while it has sent an `r` within
//! [`ACTIVE_FOR`], so a seat whose federation stalls stops holding the window
//! open. fedimintd commits alone if no answer comes within its own timeout,
//! so the coordinator can only delay a commit, never block it.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::oneshot;
use tracing::{debug, warn};

/// File name of the coordinator's socket in the MG data root.
const SOCKET_FILE: &str = "backup-sync.sock";

/// How long the coordinator holds the first waiting seat's commit for the
/// others. Rounds are 50 ms; this delays a unit's broadcast by at most this.
const WINDOW: Duration = Duration::from_millis(10);

/// A seat counts towards "everyone is waiting" while its last `r` is this
/// recent: a few rounds.
const ACTIVE_FOR: Duration = Duration::from_millis(250);

/// fedimintd's round delay, which the phase is an offset into.
const ROUND_PERIOD: Duration = Duration::from_millis(50);

const READY: u8 = b'r';
const GO: u8 = b'g';

/// The running coordinator, which every seat of this daemon joins.
struct Joined {
    socket: PathBuf,
    phase: Duration,
}

static JOINED: OnceLock<Joined> = OnceLock::new();

/// The environment that makes a seat's fedimintd join this daemon's
/// coordinator: none while no coordinator runs, as in tests that spawn seats
/// without a daemon.
pub fn seat_env() -> Vec<(&'static str, OsString)> {
    JOINED.get().map_or_else(Vec::new, |joined| {
        vec![
            (
                fedimint_core::envs::FM_BACKUP_SYNC_COORDINATOR_ENV,
                joined.socket.clone().into_os_string(),
            ),
            (
                fedimint_core::envs::FM_BACKUP_SYNC_PHASE_US_ENV,
                joined.phase.as_micros().to_string().into(),
            ),
        ]
    })
}

/// Binds the coordinator socket in `data_root` and serves it for the life of
/// the daemon; seats spawned afterwards join it ([`seat_env`]).
///
/// Call once, with the data-root lock held: it replaces a socket left by an
/// earlier daemon.
pub fn spawn(data_root: &Path) -> io::Result<()> {
    let path = data_root.join(SOCKET_FILE);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let listener = UnixListener::bind(&path)?;
    tokio::spawn(serve(listener));
    let phase = Duration::from_micros(rand::random::<u64>() % ROUND_PERIOD.as_micros() as u64);
    JOINED
        .set(Joined {
            socket: path,
            phase,
        })
        .map_err(|_| io::Error::other("the backup sync coordinator already runs"))?;
    Ok(())
}

async fn serve(listener: UnixListener) {
    let coordinator = Arc::new(Mutex::new(Coordinator::default()));
    let mut next_seat = 0_u64;
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let seat = next_seat;
                next_seat += 1;
                tokio::spawn(serve_seat(coordinator.clone(), seat, stream));
            }
            Err(error) => {
                warn!(%error, "backup sync coordinator accept failed");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

async fn serve_seat(coordinator: Arc<Mutex<Coordinator>>, seat: u64, mut stream: UnixStream) {
    let mut byte = [0; 1];
    while let Ok(1..) = stream.read(&mut byte).await {
        if byte[0] != READY {
            debug!(
                byte = byte[0],
                "backup sync coordinator got an unknown byte"
            );
            break;
        }
        let (go, released) = oneshot::channel();
        let window = coordinator
            .lock()
            .expect("coordinator lock")
            .ready(seat, go, Instant::now());
        if let Some(window) = window {
            let coordinator = coordinator.clone();
            tokio::spawn(async move {
                tokio::time::sleep(WINDOW).await;
                coordinator.lock().expect("coordinator lock").close(window);
            });
        }
        if released.await.is_err() || stream.write_all(&[GO]).await.is_err() {
            break;
        }
    }
    coordinator
        .lock()
        .expect("coordinator lock")
        .active
        .remove(&seat);
}

/// Which seats wait in the open window.
#[derive(Default)]
struct Coordinator {
    /// When each connected seat last said it was ready.
    active: HashMap<u64, Instant>,
    waiting: Vec<oneshot::Sender<()>>,
    /// Counts windows, so a window's timer can't close a later one.
    window: u64,
}

impl Coordinator {
    /// Registers a waiting seat. Returns the window to close after [`WINDOW`]
    /// when this seat opened it.
    fn ready(&mut self, seat: u64, go: oneshot::Sender<()>, now: Instant) -> Option<u64> {
        self.active.insert(seat, now);
        self.active
            .retain(|_, last_ready| now.duration_since(*last_ready) < ACTIVE_FOR);
        self.waiting.push(go);
        if self.active.len() <= self.waiting.len() {
            self.close(self.window);
            None
        } else if self.waiting.len() == 1 {
            Some(self.window)
        } else {
            None
        }
    }

    /// Releases every waiting seat, if `window` is still the open one.
    fn close(&mut self, window: u64) {
        if window != self.window {
            return;
        }
        for go in self.waiting.drain(..) {
            let _ = go.send(());
        }
        self.window += 1;
    }
}

#[cfg(test)]
#[path = "../tests/backup_sync.rs"]
mod tests;
