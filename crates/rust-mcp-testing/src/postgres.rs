//! Postgres `testcontainers` helpers for integration tests.

use std::time::Duration;

use anyhow::{Context as _, Result};
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::runners::AsyncRunner as _;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt as _};
use tokio::time::sleep;

use crate::env;

/// Running Postgres test container plus its connection URL.
#[derive(Debug)]
pub struct PostgresTestContainer {
    container: ContainerAsync<Postgres>,
    connection_string: String,
}

impl PostgresTestContainer {
    /// Starts a Postgres test container and returns its connection details.
    pub async fn start() -> Result<Self> {
        let image_tag = env::optional_env_non_empty(env::vars::RUST_MCP_TEST_POSTGRES_TAG)
            .unwrap_or_else(|| env::defaults::POSTGRES_IMAGE_TAG.to_string());

        let container = Postgres::default()
            .with_user("postgres")
            .with_password("postgres")
            .with_db_name("postgres")
            // Migrations rely on generated stored columns (Postgres 12+).
            .with_tag(image_tag.as_str())
            .start()
            .await
            .context("failed to start Postgres test container")?;

        let host = container
            .get_host()
            .await
            .context("failed to resolve Postgres test container host")?;

        // `get_host_port_ipv4` queries the Docker daemon API which can be
        // transiently unavailable under parallel test load.  Retry a few times
        // before giving up.
        let port = retry_port_lookup(&container, 5432, 10, Duration::from_millis(250))
            .await
            .context("failed to resolve mapped Postgres port after retries")?;

        let connection_string =
            format!("postgres://postgres:postgres@{host}:{port}/postgres?sslmode=disable");

        Ok(Self { container, connection_string })
    }

    /// Returns a libpq-compatible connection string for SQLx.
    pub fn connection_string(&self) -> &str {
        &self.connection_string
    }

    /// Returns the underlying container handle.
    pub fn container(&self) -> &ContainerAsync<Postgres> {
        &self.container
    }
}

async fn retry_port_lookup(
    container: &ContainerAsync<Postgres>,
    internal_port: u16,
    max_attempts: u32,
    delay: Duration,
) -> Result<u16, testcontainers_modules::testcontainers::TestcontainersError> {
    let mut last_err = None;
    for attempt in 1..=max_attempts {
        match container
            .get_host_port_ipv4(internal_port)
            .await
        {
            Ok(port) => return Ok(port),
            Err(err) => {
                eprintln!(
                    "[testcontainers] port lookup attempt {attempt}/{max_attempts} failed, \
                     retrying: {err}"
                );
                last_err = Some(err);
                if attempt < max_attempts {
                    sleep(delay).await;
                }
            }
        }
    }
    Err(last_err.unwrap())
}
