//! Postgres `testcontainers` helpers for integration tests.

use anyhow::{Context as _, Result};
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::runners::AsyncRunner as _;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt as _};

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
        let port = container
            .get_host_port_ipv4(5432)
            .await
            .context("failed to resolve mapped Postgres port")?;

        let connection_string = format!("postgres://postgres:postgres@{host}:{port}/postgres");

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
