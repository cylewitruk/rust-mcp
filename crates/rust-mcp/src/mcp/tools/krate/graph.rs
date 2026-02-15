use std::collections::{HashMap, HashSet};

use rmcp::{Json, schemars};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::mcp::models::{
    ConfidenceAssessment, ConfidenceLevel, CrateCoreRow, CrateVersionSelectionRow,
    ResponseFreshnessSource,
};
use crate::mcp::server::McpServer;
use crate::mcp::utils::{graph_depth, normalize_optional, normalize_required};

#[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CrateGraphDirection {
    Dependencies,
    Dependents,
    Both,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CrateGraphRequest {
    pub crate_name: String,
    pub version: Option<String>,
    pub direction: Option<CrateGraphDirection>,
    pub depth: Option<u32>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateGraphResponse {
    pub crate_name: String,
    pub selected_version: String,
    pub direction: CrateGraphDirection,
    pub depth: u32,
    pub node_count: usize,
    pub edge_count: usize,
    pub nodes: Vec<CrateGraphNode>,
    pub edges: Vec<CrateGraphEdge>,
    pub cycle_safe_traversal_notes: Vec<String>,
    pub freshness_check_performed: bool,
    pub freshness_check_result: String,
    pub refresh_enqueued: bool,
    pub refresh_job_id: Option<String>,
    pub freshness: Vec<ResponseFreshnessSource>,
    pub confidence: String,
    pub confidence_assessment: ConfidenceAssessment,
    pub next_best_calls: Vec<String>,
    pub provenance: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateGraphNode {
    pub crate_name: String,
    pub latest_version: Option<String>,
    pub min_distance: u32,
    pub role: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CrateGraphEdge {
    pub from_crate: String,
    pub from_version: Option<String>,
    pub to_crate: String,
    pub to_version: Option<String>,
    pub requirement: String,
    pub dependency_kind: String,
    pub optional: bool,
    pub depth: u32,
}

#[derive(Debug, FromRow)]
pub(crate) struct GraphLatestVersionRow {
    pub(crate) crate_id: i64,
    pub(crate) id: i64,
    pub(crate) version: String,
}

#[derive(Debug, FromRow)]
pub(crate) struct GraphDependencyTraversalRow {
    pub(crate) from_crate_name: String,
    pub(crate) from_version: String,
    pub(crate) to_crate_id: i64,
    pub(crate) to_crate_name: String,
    pub(crate) requirement: String,
    pub(crate) dependency_kind: String,
    pub(crate) optional: bool,
}

#[derive(Debug, FromRow)]
pub(crate) struct GraphDependentTraversalRow {
    pub(crate) from_crate_id: i64,
    pub(crate) from_crate_name: String,
    pub(crate) from_version: String,
    pub(crate) to_crate_name: String,
    pub(crate) requirement: String,
    pub(crate) dependency_kind: String,
    pub(crate) optional: bool,
}

fn detect_cycle_presence(edges: &[CrateGraphEdge]) -> bool {
    let mut adjacency = HashMap::<&str, HashSet<&str>>::new();
    for edge in edges {
        adjacency
            .entry(edge.from_crate.as_str())
            .or_default()
            .insert(edge.to_crate.as_str());
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Color {
        White,
        Gray,
        Black,
    }

    fn visit<'a>(
        node: &'a str,
        adjacency: &HashMap<&'a str, HashSet<&'a str>>,
        colors: &mut HashMap<&'a str, Color>,
    ) -> bool {
        colors.insert(node, Color::Gray);

        if let Some(neighbors) = adjacency.get(node) {
            for neighbor in neighbors {
                match colors
                    .get(neighbor)
                    .copied()
                    .unwrap_or(Color::White)
                {
                    Color::Gray => return true,
                    Color::Black => {}
                    Color::White => {
                        if visit(neighbor, adjacency, colors) {
                            return true;
                        }
                    }
                }
            }
        }

        colors.insert(node, Color::Black);
        false
    }

    let mut colors = HashMap::<&str, Color>::new();
    for node in adjacency.keys().copied() {
        if colors
            .get(node)
            .copied()
            .unwrap_or(Color::White)
            == Color::White
            && visit(node, &adjacency, &mut colors)
        {
            return true;
        }
    }

    false
}

fn cycle_safe_traversal_notes(edges: &[CrateGraphEdge], depth: u32) -> Vec<String> {
    let mut notes = vec![
        "Traversal is cycle-safe: edges are deduplicated and traversal depth is bounded."
            .to_string(),
        format!("Traversal depth cap applied: {depth}"),
    ];

    if edges
        .iter()
        .any(|edge| edge.from_crate == edge.to_crate)
    {
        notes.push("Self-referential edges detected in result set.".to_string());
    }

    if detect_cycle_presence(edges) {
        notes.push(
            "Cycle detected in returned graph; traversal remained bounded and avoided infinite \
             loops."
                .to_string(),
        );
    } else {
        notes.push("No directed cycles detected in returned graph slice.".to_string());
    }

    notes
}

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

    pub(crate) async fn handle_crate_graph(
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
        let cycle_notes = cycle_safe_traversal_notes(&edges, depth);
        let confidence_assessment = if edges.is_empty() {
            ConfidenceAssessment {
                level: ConfidenceLevel::Low,
                reason: "no dependency/dependent edges found for selected traversal slice"
                    .to_string(),
            }
        } else {
            ConfidenceAssessment {
                level: ConfidenceLevel::Medium,
                reason: "graph is depth-bounded and may omit relationships beyond traversal limit"
                    .to_string(),
            }
        };

        Ok(Json(CrateGraphResponse {
            crate_name: crate_row.name,
            selected_version: selected_version.version,
            direction,
            depth,
            node_count: nodes.len(),
            edge_count: edges.len(),
            nodes,
            edges,
            cycle_safe_traversal_notes: cycle_notes,
            freshness_check_performed: freshness_outcome.freshness_check_performed,
            freshness_check_result: freshness_check_result.clone(),
            refresh_enqueued: freshness_outcome.refresh_enqueued,
            refresh_job_id: freshness_outcome.refresh_job_id,
            freshness: vec![
                ResponseFreshnessSource {
                    source: "local_postgres_index".to_string(),
                    status: "fresh".to_string(),
                    checked_at: crate_row.updated_at,
                },
                ResponseFreshnessSource {
                    source: "crates.io".to_string(),
                    status: freshness_check_result,
                    checked_at: None,
                },
            ],
            confidence: confidence_assessment
                .level
                .as_str()
                .to_string(),
            confidence_assessment,
            next_best_calls: vec![
                "crate.intel".to_string(),
                "crate.versions".to_string(),
                "index.refresh".to_string(),
            ],
            provenance: "local_postgres_index".to_string(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{CrateGraphEdge, cycle_safe_traversal_notes, detect_cycle_presence};

    fn edge(from: &str, to: &str) -> CrateGraphEdge {
        CrateGraphEdge {
            from_crate: from.to_string(),
            from_version: Some("1.0.0".to_string()),
            to_crate: to.to_string(),
            to_version: Some("1.0.0".to_string()),
            requirement: "*".to_string(),
            dependency_kind: "normal".to_string(),
            optional: false,
            depth: 1,
        }
    }

    #[test]
    fn cycle_detection_identifies_simple_cycle() {
        let edges = vec![edge("a", "b"), edge("b", "c"), edge("c", "a")];
        assert!(detect_cycle_presence(&edges));
    }

    #[test]
    fn cycle_notes_include_no_cycle_message_when_acyclic() {
        let edges = vec![edge("a", "b"), edge("b", "c")];
        let notes = cycle_safe_traversal_notes(&edges, 2);
        assert!(
            notes
                .iter()
                .any(|note| note.contains("No directed cycles detected"))
        );
    }
}
