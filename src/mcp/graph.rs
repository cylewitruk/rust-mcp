use std::collections::{HashMap, HashSet};

use rmcp::Json;

use super::models::{
    CrateCoreRow, CrateGraphDirection, CrateGraphEdge, CrateGraphNode, CrateGraphRequest,
    CrateGraphResponse, CrateVersionSelectionRow, GraphDependencyTraversalRow,
    GraphDependentTraversalRow, GraphLatestVersionRow,
};
use super::server::McpServer;
use super::utils::{graph_depth, normalize_optional, normalize_required};

impl McpServer {
    async fn latest_versions_for_crates(
        &self,
        crate_ids: &[i64],
    ) -> Result<HashMap<i64, (i64, String)>, String> {
        if crate_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let rows = sqlx::query_as::<_, GraphLatestVersionRow>(
            "SELECT DISTINCT ON (crate_id)
                crate_id,
                id,
                version
             FROM crate_versions
             WHERE crate_id = ANY($1)
             ORDER BY crate_id, published_at DESC NULLS LAST, id DESC",
        )
        .bind(crate_ids)
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| format!("latest-version lookup failed: {e}"))?;

        Ok(rows
            .into_iter()
            .map(|row| (row.crate_id, (row.id, row.version)))
            .collect::<HashMap<_, _>>())
    }

    pub(super) async fn handle_crate_graph(
        &self,
        request: CrateGraphRequest,
    ) -> Result<Json<CrateGraphResponse>, String> {
        let crate_name = normalize_required(request.crate_name, "crate_name")?;
        let requested_version = normalize_optional(request.version);
        let direction = request
            .direction
            .unwrap_or(CrateGraphDirection::Dependencies);
        let depth = graph_depth(request.depth);

        let crate_row = sqlx::query_as::<_, CrateCoreRow>(
            "SELECT
                id,
                name,
                description,
                repository_url,
                docs_url,
                homepage_url,
                categories,
                keywords,
                updated_at::TEXT AS updated_at
             FROM crates
             WHERE name = $1",
        )
        .bind(&crate_name)
        .fetch_optional(&self.state.db)
        .await
        .map_err(|e| format!("crate lookup failed for {crate_name}: {e}"))?
        .ok_or_else(|| {
            format!("crate '{crate_name}' is not indexed locally; run index.sync_crates first")
        })?;

        let latest_version = sqlx::query_as::<_, CrateVersionSelectionRow>(
            "SELECT
                id,
                version,
                published_at::TEXT AS published_at,
                readme
             FROM crate_versions
             WHERE crate_id = $1
             ORDER BY published_at DESC NULLS LAST, id DESC
             LIMIT 1",
        )
        .bind(crate_row.id)
        .fetch_optional(&self.state.db)
        .await
        .map_err(|e| format!("latest version lookup failed for {crate_name}: {e}"))?
        .ok_or_else(|| {
            format!(
                "crate '{}' has no indexed versions yet; run index.sync_crates first",
                crate_row.name
            )
        })?;

        let freshness_outcome = self
            .ensure_freshness_for_interaction(
                crate_row.id,
                &crate_row.name,
                &latest_version.version,
            )
            .await?;

        let selected_version = if let Some(version) = requested_version {
            sqlx::query_as::<_, CrateVersionSelectionRow>(
                "SELECT
                    id,
                    version,
                    published_at::TEXT AS published_at,
                    readme
                 FROM crate_versions
                 WHERE crate_id = $1 AND version = $2
                 LIMIT 1",
            )
            .bind(crate_row.id)
            .bind(&version)
            .fetch_optional(&self.state.db)
            .await
            .map_err(|e| {
                format!("selected version lookup failed for {}@{}: {e}", crate_row.name, version)
            })?
            .ok_or_else(|| {
                format!(
                    "version '{}' for crate '{}' is not indexed locally",
                    version, crate_row.name
                )
            })?
        } else {
            latest_version.clone()
        };

        let mut edges = Vec::<CrateGraphEdge>::new();
        let mut node_min_distance = HashMap::<String, u32>::new();
        let mut node_latest_version = HashMap::<String, Option<String>>::new();
        let mut edge_seen = HashSet::<(String, String, String, String, bool, u32)>::new();

        node_min_distance.insert(crate_row.name.clone(), 0);
        node_latest_version.insert(crate_row.name.clone(), Some(latest_version.version.clone()));

        if matches!(direction, CrateGraphDirection::Dependencies | CrateGraphDirection::Both) {
            let mut frontier_versions = vec![selected_version.id];

            for depth_level in 1..=depth {
                if frontier_versions.is_empty() {
                    break;
                }

                let rows = sqlx::query_as::<_, GraphDependencyTraversalRow>(
                    "SELECT
                        c_from.name AS from_crate_name,
                        cv_from.version AS from_version,
                        d.to_crate_id AS to_crate_id,
                        c_to.name AS to_crate_name,
                        d.requirement,
                        d.dependency_kind,
                        d.optional
                     FROM dependency_edges d
                     JOIN crate_versions cv_from ON cv_from.id = d.from_version_id
                     JOIN crates c_from ON c_from.id = cv_from.crate_id
                     JOIN crates c_to ON c_to.id = d.to_crate_id
                     WHERE d.from_version_id = ANY($1)",
                )
                .bind(&frontier_versions)
                .fetch_all(&self.state.db)
                .await
                .map_err(|e| format!("dependency traversal failed: {e}"))?;

                let next_crate_ids = rows
                    .iter()
                    .map(|row| row.to_crate_id)
                    .collect::<Vec<_>>();
                let next_latest = self
                    .latest_versions_for_crates(&next_crate_ids)
                    .await?;

                let mut next_versions = Vec::<i64>::new();
                for row in rows {
                    let to_latest_version = next_latest
                        .get(&row.to_crate_id)
                        .map(|(_, v)| v.clone());

                    node_min_distance
                        .entry(row.from_crate_name.clone())
                        .and_modify(|d| *d = (*d).min(depth_level.saturating_sub(1)))
                        .or_insert(depth_level.saturating_sub(1));
                    node_min_distance
                        .entry(row.to_crate_name.clone())
                        .and_modify(|d| *d = (*d).min(depth_level))
                        .or_insert(depth_level);

                    node_latest_version
                        .entry(row.from_crate_name.clone())
                        .or_insert(Some(row.from_version.clone()));
                    node_latest_version
                        .entry(row.to_crate_name.clone())
                        .or_insert(to_latest_version.clone());

                    let edge_key = (
                        row.from_crate_name.clone(),
                        row.to_crate_name.clone(),
                        row.requirement.clone(),
                        row.dependency_kind.clone(),
                        row.optional,
                        depth_level,
                    );
                    if edge_seen.insert(edge_key) {
                        edges.push(CrateGraphEdge {
                            from_crate: row.from_crate_name,
                            from_version: Some(row.from_version),
                            to_crate: row.to_crate_name,
                            to_version: to_latest_version,
                            requirement: row.requirement,
                            dependency_kind: row.dependency_kind,
                            optional: row.optional,
                            depth: depth_level,
                        });
                    }

                    if let Some((next_id, _)) = next_latest.get(&row.to_crate_id) {
                        next_versions.push(*next_id);
                    }
                }

                next_versions.sort_unstable();
                next_versions.dedup();
                frontier_versions = next_versions;
            }
        }

        if matches!(direction, CrateGraphDirection::Dependents | CrateGraphDirection::Both) {
            let mut frontier_crates = vec![crate_row.id];

            for depth_level in 1..=depth {
                if frontier_crates.is_empty() {
                    break;
                }

                let rows = sqlx::query_as::<_, GraphDependentTraversalRow>(
                    "SELECT
                        cv_from.crate_id AS from_crate_id,
                        c_from.name AS from_crate_name,
                        cv_from.version AS from_version,
                        c_to.name AS to_crate_name,
                        d.requirement,
                        d.dependency_kind,
                        d.optional
                     FROM dependency_edges d
                     JOIN crate_versions cv_from ON cv_from.id = d.from_version_id
                     JOIN crates c_from ON c_from.id = cv_from.crate_id
                     JOIN crates c_to ON c_to.id = d.to_crate_id
                     WHERE d.to_crate_id = ANY($1)",
                )
                .bind(&frontier_crates)
                .fetch_all(&self.state.db)
                .await
                .map_err(|e| format!("dependent traversal failed: {e}"))?;

                let next_crate_ids = rows
                    .iter()
                    .map(|row| row.from_crate_id)
                    .collect::<Vec<_>>();
                let next_latest = self
                    .latest_versions_for_crates(&next_crate_ids)
                    .await?;

                let mut next_frontier = Vec::<i64>::new();
                for row in rows {
                    let from_latest_version = next_latest
                        .get(&row.from_crate_id)
                        .map(|(_, v)| v.clone());

                    node_min_distance
                        .entry(row.to_crate_name.clone())
                        .and_modify(|d| *d = (*d).min(depth_level.saturating_sub(1)))
                        .or_insert(depth_level.saturating_sub(1));
                    node_min_distance
                        .entry(row.from_crate_name.clone())
                        .and_modify(|d| *d = (*d).min(depth_level))
                        .or_insert(depth_level);

                    node_latest_version
                        .entry(row.to_crate_name.clone())
                        .or_insert(Some(
                            selected_version
                                .version
                                .clone(),
                        ));
                    node_latest_version
                        .entry(row.from_crate_name.clone())
                        .or_insert(from_latest_version.clone());

                    let edge_key = (
                        row.from_crate_name.clone(),
                        row.to_crate_name.clone(),
                        row.requirement.clone(),
                        row.dependency_kind.clone(),
                        row.optional,
                        depth_level,
                    );
                    if edge_seen.insert(edge_key) {
                        edges.push(CrateGraphEdge {
                            from_crate: row.from_crate_name,
                            from_version: Some(row.from_version),
                            to_crate: row.to_crate_name,
                            to_version: Some(
                                selected_version
                                    .version
                                    .clone(),
                            ),
                            requirement: row.requirement,
                            dependency_kind: row.dependency_kind,
                            optional: row.optional,
                            depth: depth_level,
                        });
                    }

                    if let Some((next_id, _)) = next_latest.get(&row.from_crate_id) {
                        let _ = next_id;
                    }
                    next_frontier.push(row.from_crate_id);
                }

                next_frontier.sort_unstable();
                next_frontier.dedup();
                frontier_crates = next_frontier;
            }
        }

        let mut nodes = node_min_distance
            .into_iter()
            .map(|(name, min_distance)| {
                let role =
                    if name == crate_row.name { "root".to_string() } else { "related".to_string() };
                CrateGraphNode {
                    latest_version: node_latest_version
                        .get(&name)
                        .cloned()
                        .flatten(),
                    crate_name: name,
                    min_distance,
                    role,
                }
            })
            .collect::<Vec<_>>();
        nodes.sort_by(|a, b| {
            a.min_distance
                .cmp(&b.min_distance)
                .then(
                    a.crate_name
                        .cmp(&b.crate_name),
                )
        });

        let freshness_check_result = freshness_outcome
            .freshness_check_result
            .clone();

        Ok(Json(CrateGraphResponse {
            crate_name: crate_row.name,
            selected_version: selected_version.version,
            direction,
            depth,
            node_count: nodes.len(),
            edge_count: edges.len(),
            nodes,
            edges,
            freshness_check_performed: freshness_outcome.freshness_check_performed,
            freshness_check_result: freshness_check_result.clone(),
            refresh_enqueued: freshness_outcome.refresh_enqueued,
            refresh_job_id: freshness_outcome.refresh_job_id,
            freshness: vec![
                super::models::ResponseFreshnessSource {
                    source: "local_postgres_index".to_string(),
                    status: "fresh".to_string(),
                    checked_at: crate_row.updated_at,
                },
                super::models::ResponseFreshnessSource {
                    source: "crates.io".to_string(),
                    status: freshness_check_result,
                    checked_at: None,
                },
            ],
            confidence: "medium".to_string(),
            next_best_calls: vec![
                "crate.intel".to_string(),
                "crate.versions".to_string(),
                "index.refresh".to_string(),
            ],
            provenance: "local_postgres_index".to_string(),
        }))
    }
}
