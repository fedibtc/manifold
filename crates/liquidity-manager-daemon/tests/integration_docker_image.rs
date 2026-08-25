use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, ensure};
use reqwest::{Client, StatusCode};
use serde_json::Value;
use tracing_subscriber::EnvFilter;

const ADMIN_TOKEN: &str = "flip-local-admin-token";

fn init_logging() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new("fedi_decentralized_liquidity_manager_daemon=debug,info")
        }))
        .with_test_writer()
        .try_init();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "packaging smoke test; requires a local Docker daemon"]
async fn docker_image_starts_and_exposes_private_health() -> anyhow::Result<()> {
    init_logging();

    let workspace = workspace_root();
    let temp = TestDataDir::new("integration-docker-image")?;
    let image_tar = temp.path().join("flip-image");
    run(Command::new("nix")
        .arg("build")
        .arg("path:.#liquidityManagerDockerImage")
        .arg("-o")
        .arg(&image_tar)
        .arg("--no-write-lock-file")
        .current_dir(&workspace))
    .context("build FLIP Docker image with Nix")?;
    let loaded = run(Command::new("docker")
        .arg("load")
        .arg("--input")
        .arg(&image_tar))
    .context("load FLIP Docker image into Docker")?;
    // The image is tagged with the workspace version, so take the reference
    // `docker load` reports rather than restating the tag here and having it
    // drift on the next version bump.
    let image_name = loaded_image_name(&loaded)?;

    let ports = TestPorts::allocate()?;
    let container_name = format!("flip-phase10-smoke-{}", unique_suffix());
    let container = DockerContainer::run(&container_name, temp.path(), &ports, &image_name)?;
    let admin_url = format!("http://{}", ports.admin_bind_address);
    let client = Client::new();

    wait_for_health(&client, &admin_url).await?;
    let unauth_status = client
        .post(format!("{admin_url}/admin/v1/get_health"))
        .send()
        .await?
        .status();
    assert_eq!(unauth_status, StatusCode::UNAUTHORIZED);

    let health: Value = client
        .post(format!("{admin_url}/admin/v1/get_health"))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(health["overall_status"], "healthy");
    wait_for_container_health(&container.name).await?;

    drop(container);
    Ok(())
}

struct TestPorts {
    admin_bind_address: SocketAddr,
    public_bind_address: SocketAddr,
}

impl TestPorts {
    fn allocate() -> anyhow::Result<Self> {
        let base_port = defe_portalloc::port_alloc(2).context("allocate Docker test ports")?;
        let public_port = base_port
            .checked_add(1)
            .context("allocated Docker test port range overflowed")?;
        Ok(Self {
            admin_bind_address: SocketAddr::from(([127, 0, 0, 1], base_port)),
            public_bind_address: SocketAddr::from(([127, 0, 0, 1], public_port)),
        })
    }
}

struct DockerContainer {
    name: String,
}

impl DockerContainer {
    fn run(
        name: &str,
        data_dir: &Path,
        ports: &TestPorts,
        image_name: &str,
    ) -> anyhow::Result<Self> {
        let output = run(Command::new("docker")
            .arg("run")
            .arg("--rm")
            .arg("--detach")
            .arg("--name")
            .arg(name)
            .arg("--pull")
            .arg("never")
            .arg("-e")
            .arg(format!("FLIP_BOOTSTRAP_ADMIN_TOKEN={ADMIN_TOKEN}"))
            .arg("-p")
            .arg(format!("{}:8173", ports.admin_bind_address))
            .arg("-p")
            // The public Liquidity API is Iroh over QUIC (UDP); a bare
            // mapping would publish TCP and exercise nothing.
            .arg(format!("{}:8174/udp", ports.public_bind_address))
            .arg("-v")
            .arg(format!("{}:/var/lib/flip", data_dir.display()))
            .arg(image_name))
        .context("start FLIP Docker image")?;
        let container_id = String::from_utf8_lossy(&output.stdout);
        ensure!(
            !container_id.trim().is_empty(),
            "docker run did not return a container id"
        );
        Ok(Self {
            name: name.to_owned(),
        })
    }
}

impl Drop for DockerContainer {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .arg("rm")
            .arg("--force")
            .arg(&self.name)
            .output();
    }
}

struct TestDataDir {
    path: PathBuf,
}

impl TestDataDir {
    fn new(name: &str) -> anyhow::Result<Self> {
        let path = std::env::temp_dir()
            .join("fedi-flip-tests")
            .join(format!("{name}-{}", unique_suffix()));
        fs::create_dir_all(&path)
            .with_context(|| format!("create test data dir {}", path.display()))?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDataDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Waits until the daemon can actually serve an Admin API request.
///
/// Deliberately not a poll of unauthenticated `/health`. That answers 200 while
/// the process has no runtime generation, but every `/admin/v1/*` route answers
/// 503 until the generation is installed, and the Admin listener binds before
/// it — so waiting on `/health` races daemon startup and the caller's first
/// admin call can land inside that window.
async fn wait_for_health(client: &Client, admin_url: &str) -> anyhow::Result<()> {
    for _ in 0..60 {
        if let Ok(response) = client
            .post(format!("{admin_url}/admin/v1/get_health"))
            .bearer_auth(ADMIN_TOKEN)
            .json(&serde_json::json!({}))
            .send()
            .await
            && response.status() == StatusCode::OK
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    anyhow::bail!("Dockerized daemon did not begin serving Admin API requests at {admin_url}")
}

async fn wait_for_container_health(container_name: &str) -> anyhow::Result<()> {
    for _ in 0..30 {
        let output = run(Command::new("docker")
            .arg("inspect")
            .arg("--format")
            .arg("{{.State.Health.Status}}")
            .arg(container_name))?;
        let status = String::from_utf8_lossy(&output.stdout);
        if status.trim() == "healthy" {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    anyhow::bail!("Docker healthcheck for {container_name} did not become healthy")
}

/// Extract the `name:tag` reference from `docker load` output, which reports
/// `Loaded image: <name>:<tag>` for each image in the archive.
fn loaded_image_name(loaded: &Output) -> anyhow::Result<String> {
    let stdout = String::from_utf8_lossy(&loaded.stdout);
    let image_name = stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("Loaded image: "))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .with_context(|| format!("docker load did not report a loaded image:\n{stdout}"))?;
    Ok(image_name.to_owned())
}

fn run(command: &mut Command) -> anyhow::Result<Output> {
    let output = command.output()?;
    if output.status.success() {
        return Ok(output);
    }
    anyhow::bail!(
        "command failed with status {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("daemon crate lives under crates/liquidity-manager-daemon")
        .to_path_buf()
}

fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after Unix epoch")
        .as_nanos();
    format!("{}-{nanos}", std::process::id())
}
