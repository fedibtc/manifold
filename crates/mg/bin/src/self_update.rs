//! Automatic update to a threshold-signed release.
//!
//! The setup-payment publication carries the latest MG release as
//! `mg_release`. When its version is above this binary's, enough release
//! signers pinned by the environment profile signed it, and the MG owner has
//! not turned updates off, the [`Updater`] waits a random delay, downloads the
//! static binary for this CPU into `<data-dir>/releases/`, checks its hash, and
//! records it as staged. The daemon then shuts down gracefully and
//! [`exec_staged`] replaces this process with the staged binary.
//!
//! Every daemon start first runs [`staged_binary`]: a staged release newer than
//! the running binary replaces it before the daemon opens anything. A platform
//! update that ships a newer binary than the staged one therefore wins, and
//! updates only ever move forward.

#[cfg(test)]
mod tests;

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher as _, Hasher as _};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context as _;
use fedi_decentralized_domain::{
    AdmittedSetupPaymentFederations, MgReleaseArch, MgReleaseBinary, MgVersion, Sha256Hash,
};
use fedi_decentralized_manifold_environment::{ManifoldEnvironment, MgReleaseSigners};
use sha2::{Digest as _, Sha256};
use tokio::sync::watch;

/// Directory under the data root holding staged release binaries.
const RELEASES_DIR: &str = "releases";

/// Record of the staged release, written atomically after its binary.
const STAGED_FILE: &str = "staged.json";

/// Wait between attempts while a download or install keeps failing.
const RETRY_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// Longest a download may take before it is retried.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// The version of this binary.
pub fn running_version() -> MgVersion {
    env!("CARGO_PKG_VERSION")
        .parse()
        .expect("Cargo package version is SemVer")
}

/// The longest random delay before an MG installs a release. All MGs see a
/// publication at about the same time; the delay keeps every guardian of a
/// federation from restarting at once.
pub fn max_delay(environment: ManifoldEnvironment) -> Duration {
    match environment {
        ManifoldEnvironment::Development => Duration::from_secs(5),
        ManifoldEnvironment::Staging | ManifoldEnvironment::Production => {
            Duration::from_secs(15 * 60)
        }
    }
}

/// The staged release: its version and the hash that names its binary.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct Staged {
    version: MgVersion,
    sha256: Sha256Hash,
}

/// `<data-dir>/releases/`: staged binaries and the record of the current one.
#[derive(Clone, Debug)]
pub struct Releases {
    dir: PathBuf,
}

impl Releases {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join(RELEASES_DIR),
        }
    }

    fn binary_path(&self, sha256: &Sha256Hash) -> PathBuf {
        self.dir.join(sha256.to_string())
    }

    fn staged(&self) -> anyhow::Result<Option<Staged>> {
        match std::fs::read(self.dir.join(STAGED_FILE)) {
            Ok(bytes) => Ok(Some(
                serde_json::from_slice(&bytes).context("parse staged release record")?,
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).context("read staged release record"),
        }
    }

    /// Download, verify, and stage one release binary.
    ///
    /// The binary is complete and synced under its hash name before the staged
    /// record names it, so a crash leaves either the old record or a record of
    /// a complete binary.
    async fn install(
        &self,
        version: &MgVersion,
        binary: &MgReleaseBinary,
        client: &reqwest::Client,
    ) -> anyhow::Result<()> {
        tokio::fs::create_dir_all(&self.dir)
            .await
            .context("create releases directory")?;
        let partial = self.dir.join(format!("{}.partial", binary.sha256));
        download(client, binary, &partial).await?;
        let path = self.binary_path(&binary.sha256);
        tokio::fs::rename(&partial, &path)
            .await
            .context("move release binary into place")?;
        let staged = Staged {
            version: version.clone(),
            sha256: binary.sha256,
        };
        let dir = self.dir.clone();
        tokio::task::spawn_blocking(move || {
            write_atomically(&dir, STAGED_FILE, &serde_json::to_vec(&staged)?)
        })
        .await??;
        Ok(())
    }

    /// Remove every binary except the staged one and the running one. Best
    /// effort: a leftover file only costs disk space.
    ///
    /// A staged release that is no newer than the running binary, and is not
    /// the running binary, lost to a platform update: its record goes too, so
    /// its binary does not stay on disk until the next release.
    ///
    /// Only a daemon that is past [`staged_binary`] calls this: the running
    /// binary stays, because seats spawn their `fedimintd` from it.
    pub fn prune(&self) {
        let running = std::env::current_exe()
            .ok()
            .and_then(|path| path.canonicalize().ok());
        let staged = self.staged().ok().flatten().filter(|staged| {
            staged.version > running_version()
                || self.binary_path(&staged.sha256).canonicalize().ok() == running
        });
        if staged.is_none()
            && let Err(error) = std::fs::remove_file(self.dir.join(STAGED_FILE))
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%error, "could not remove the outdated staged release record");
        }
        let staged = staged.map(|staged| staged.sha256.to_string());
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let hash = name.strip_suffix(".partial").unwrap_or(name);
            if hash.parse::<Sha256Hash>().is_err()
                || (name == hash && staged.as_deref() == Some(hash))
                || entry.path().canonicalize().ok() == running
            {
                continue;
            }
            if let Err(error) = std::fs::remove_file(entry.path()) {
                tracing::warn!(%error, file = name, "could not remove an unused release binary");
            }
        }
    }
}

/// The staged binary to run instead of this one, if any.
///
/// It is chosen only when its release is newer than this binary, this process
/// is not already it, and its contents still have the staged hash. Anything
/// else keeps the running binary, which is the platform's own after a
/// platform update.
pub fn staged_binary(releases: &Releases) -> anyhow::Result<Option<PathBuf>> {
    let Some(staged) = releases.staged()? else {
        return Ok(None);
    };
    if staged.version <= running_version() {
        return Ok(None);
    }
    let path = releases.binary_path(&staged.sha256);
    let current = std::env::current_exe().context("locate the running binary")?;
    if current.canonicalize().ok() == path.canonicalize().ok() {
        return Ok(None);
    }
    let actual = sha256_file(&path).with_context(|| format!("hash {}", path.display()))?;
    if actual != staged.sha256 {
        anyhow::bail!(
            "staged binary {} has hash {actual}, expected {}",
            path.display(),
            staged.sha256
        );
    }
    Ok(Some(path))
}

/// Replace this process with `binary`, keeping its arguments, `argv[0]`, and
/// environment. Returns only on failure.
pub fn exec(binary: &Path) -> std::io::Error {
    use std::os::unix::process::CommandExt as _;

    let mut args = std::env::args_os();
    let mut command = std::process::Command::new(binary);
    if let Some(argv0) = args.next() {
        command.arg0(argv0);
    }
    command.args(args).exec()
}

/// What the updater decided about one published release.
#[derive(Debug, Eq, PartialEq)]
enum Decision {
    /// Nothing to install.
    Skip(&'static str),
    /// Install this binary as `version`.
    Install {
        version: MgVersion,
        binary: MgReleaseBinary,
    },
}

/// Decide whether a publication carries a release this MG should install.
fn decide(
    policy: &AdmittedSetupPaymentFederations,
    environment: ManifoldEnvironment,
    signers: &MgReleaseSigners,
    running: &MgVersion,
    staged: Option<&Staged>,
) -> Decision {
    let Some(release) = policy.mg_release() else {
        return Decision::Skip("no release published");
    };
    let version = policy.mg_version();
    if version <= running {
        return Decision::Skip("published release is not newer");
    }
    if !signers.is_satisfied_by(&release.valid_signers(&environment.to_string(), version)) {
        return Decision::Skip("release lacks the threshold of signer signatures");
    }
    let Some(binary) = release.binary(&MgReleaseArch::current().to_string()) else {
        return Decision::Skip("release has no binary for this CPU");
    };
    if staged.is_some_and(|staged| staged.sha256 == binary.sha256) {
        // Staging is followed by exec. Running older code with this release
        // staged means that exec failed; retrying would only fail again.
        return Decision::Skip("release is already staged but did not start");
    }
    Decision::Install {
        version: version.clone(),
        binary: binary.clone(),
    }
}

/// Watches the setup-payment publication and stages newer releases.
pub struct Updater {
    pub releases: Releases,
    pub environment: ManifoldEnvironment,
    pub signers: MgReleaseSigners,
    pub max_delay: Duration,
    pub policy: watch::Receiver<Option<AdmittedSetupPaymentFederations>>,
    pub enabled: watch::Receiver<bool>,
}

impl Updater {
    /// Run until a release is staged, then return so the daemon can stop and
    /// exec it. Never returns otherwise: a failure here stops updates, never
    /// the guardians.
    pub async fn run(mut self) {
        let client = match reqwest::Client::builder().timeout(DOWNLOAD_TIMEOUT).build() {
            Ok(client) => client,
            Err(error) => {
                tracing::error!(%error, "cannot build the release download client; automatic updates are off");
                return std::future::pending().await;
            }
        };
        let running = running_version();
        loop {
            let decision = if *self.enabled.borrow_and_update() {
                let staged = match self.releases.staged() {
                    Ok(staged) => staged,
                    Err(error) => {
                        tracing::warn!(%error, "ignoring unreadable staged release record");
                        None
                    }
                };
                match self.policy.borrow_and_update().as_ref() {
                    Some(policy) => decide(
                        policy,
                        self.environment,
                        &self.signers,
                        &running,
                        staged.as_ref(),
                    ),
                    None => Decision::Skip("no setup-payment publication yet"),
                }
            } else {
                Decision::Skip("automatic updates are off")
            };
            match decision {
                Decision::Skip(reason) => {
                    tracing::debug!(reason, "no MG update to install");
                    self.changed().await;
                }
                Decision::Install { version, binary } => {
                    let delay = random_delay(self.max_delay);
                    tracing::info!(
                        %version,
                        sha256 = %binary.sha256,
                        delay_secs = delay.as_secs(),
                        "MG update published; installing after a random delay"
                    );
                    // A new publication or an opt-out during the delay is
                    // decided again from the start.
                    if tokio::time::timeout(delay, self.changed()).await.is_ok() {
                        continue;
                    }
                    match self.releases.install(&version, &binary, &client).await {
                        Ok(()) => {
                            tracing::info!(%version, "MG update staged; restarting into it");
                            return;
                        }
                        Err(error) => {
                            tracing::warn!(%version, error = format!("{error:#}"), "MG update failed; retrying later");
                            let _ = tokio::time::timeout(RETRY_INTERVAL, self.changed()).await;
                        }
                    }
                }
            }
        }
    }

    /// Wait for a new publication or a change of the opt-out setting. Their
    /// owners live as long as the daemon serves; if one is gone anyway,
    /// nothing can change again, so wait forever.
    async fn changed(&mut self) {
        let closed = tokio::select! {
            result = self.policy.changed() => result.is_err(),
            result = self.enabled.changed() => result.is_err(),
        };
        if closed {
            tracing::warn!("an MG update input closed; automatic updates stop");
            std::future::pending::<()>().await;
        }
    }
}

fn random_delay(max: Duration) -> Duration {
    let millis = u64::try_from(max.as_millis()).unwrap_or(u64::MAX);
    if millis == 0 {
        return Duration::ZERO;
    }
    Duration::from_millis(RandomState::new().build_hasher().finish() % millis)
}

/// Stream `binary` into `path`, refusing more bytes than its size and any
/// hash other than its own, then make it executable and sync it.
async fn download(
    client: &reqwest::Client,
    binary: &MgReleaseBinary,
    path: &Path,
) -> anyhow::Result<()> {
    let mut response = client
        .get(&binary.url.0)
        .send()
        .await
        .context("request release binary")?
        .error_for_status()
        .context("request release binary")?;
    let mut file = std::fs::File::create(path).context("create release binary file")?;
    let mut hasher = Sha256::new();
    let mut received = 0u64;
    while let Some(chunk) = response.chunk().await.context("download release binary")? {
        received += chunk.len() as u64;
        if received > binary.size {
            anyhow::bail!(
                "release binary is larger than its published {} bytes",
                binary.size
            );
        }
        hasher.update(&chunk);
        file.write_all(&chunk).context("write release binary")?;
    }
    if received != binary.size {
        anyhow::bail!(
            "release binary has {received} bytes, published size is {}",
            binary.size
        );
    }
    let actual = Sha256Hash::new(hasher.finalize().into());
    if actual != binary.sha256 {
        anyhow::bail!(
            "release binary has hash {actual}, published {}",
            binary.sha256
        );
    }
    use std::os::unix::fs::PermissionsExt as _;
    file.set_permissions(std::fs::Permissions::from_mode(0o755))
        .context("make release binary executable")?;
    file.sync_all().context("sync release binary")?;
    Ok(())
}

fn sha256_file(path: &Path) -> std::io::Result<Sha256Hash> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(Sha256Hash::new(hasher.finalize().into()));
        }
        hasher.update(&buffer[..read]);
    }
}

fn write_atomically(dir: &Path, name: &str, contents: &[u8]) -> anyhow::Result<()> {
    let temporary = dir.join(format!("{name}.tmp"));
    let mut file = std::fs::File::create(&temporary).context("create staged release record")?;
    file.write_all(contents)
        .and_then(|()| file.sync_all())
        .context("write staged release record")?;
    std::fs::rename(&temporary, dir.join(name)).context("replace staged release record")?;
    std::fs::File::open(dir)
        .and_then(|dir| dir.sync_all())
        .context("sync releases directory")?;
    Ok(())
}
