use rmcp::Json;
use sqlx::{Postgres, QueryBuilder};

use super::models::{
    CrateSearchHit, CrateSearchRequest, CrateSearchResponse, CrateSearchRow, CrateSearchSort,
};
use super::server::McpServer;
use super::utils::{match_reasons, normalize_optional, search_limit};

impl McpServer {
    async fn run_crate_search(
        &self,
        query: Option<&str>,
        category: Option<&str>,
        keyword: Option<&str>,
        sort: CrateSearchSort,
        limit: u32,
    ) -> Result<Vec<CrateSearchRow>, sqlx::Error> {
        let mut qb = QueryBuilder::<Postgres>::new(
            "SELECT c.id AS crate_id, c.name, c.description, c.repository_url, c.docs_url, \
             c.homepage_url, c.categories, c.keywords, COALESCE(MAX(cv.total_downloads), \
             0)::BIGINT AS total_downloads, (ARRAY_REMOVE(ARRAY_AGG(cv.version ORDER BY \
             cv.published_at DESC NULLS LAST, cv.id DESC), NULL))[1] AS latest_version, \
             MAX(cv.published_at)::TEXT AS latest_published_at, COALESCE(COUNT(DISTINCT \
             dep.from_version_id), 0)::BIGINT AS dependent_count, ",
        );

        if let Some(q) = query {
            let name_like = format!("%{q}%");
            qb.push("(similarity(c.name, ");
            qb.push_bind(q);
            qb.push(") * 2.0 + similarity(COALESCE(c.description, ''), ");
            qb.push_bind(q);
            qb.push(") + CASE WHEN c.name ILIKE ");
            qb.push_bind(name_like);
            qb.push(
                " THEN 1.5 ELSE 0 END + CASE WHEN to_tsvector('english', COALESCE(c.description, \
                 '')) @@ websearch_to_tsquery('english', ",
            );
            qb.push_bind(q);
            qb.push(") THEN 1.0 ELSE 0 END)::DOUBLE PRECISION AS relevance_score ");
        } else {
            qb.push("0.0::DOUBLE PRECISION AS relevance_score ");
        }

        qb.push(
            "FROM crates c LEFT JOIN crate_versions cv ON cv.crate_id = c.id LEFT JOIN \
             dependency_edges dep ON dep.to_crate_id = c.id ",
        );

        let mut has_where = false;

        if let Some(q) = query {
            if !has_where {
                qb.push("WHERE ");
                has_where = true;
            }
            qb.push("(");
            qb.push("c.name ILIKE ");
            qb.push_bind(format!("%{q}%"));
            qb.push(" OR COALESCE(c.description, '') ILIKE ");
            qb.push_bind(format!("%{q}%"));
            qb.push(" OR COALESCE(c.keywords, '{}'::TEXT[]) @> ARRAY[");
            qb.push_bind(q);
            qb.push("]::TEXT[]");
            qb.push(" OR COALESCE(c.categories, '{}'::TEXT[]) @> ARRAY[");
            qb.push_bind(q);
            qb.push("]::TEXT[]");
            qb.push(
                " OR to_tsvector('english', COALESCE(c.description, '')) @@ \
                 websearch_to_tsquery('english', ",
            );
            qb.push_bind(q);
            qb.push(")");
            qb.push(")");
        }

        if let Some(c) = category {
            if !has_where {
                qb.push("WHERE ");
                has_where = true;
            } else {
                qb.push("AND ");
            }
            qb.push("COALESCE(c.categories, '{}'::TEXT[]) @> ARRAY[");
            qb.push_bind(c);
            qb.push("]::TEXT[] ");
        }

        if let Some(k) = keyword {
            if !has_where {
                qb.push("WHERE ");
            } else {
                qb.push("AND ");
            }
            qb.push("COALESCE(c.keywords, '{}'::TEXT[]) @> ARRAY[");
            qb.push_bind(k);
            qb.push("]::TEXT[] ");
        }

        qb.push(
            "GROUP BY c.id, c.name, c.description, c.repository_url, c.docs_url, c.homepage_url, \
             c.categories, c.keywords ",
        );

        match sort {
            CrateSearchSort::Relevance => {
                if query.is_some() {
                    qb.push("ORDER BY relevance_score DESC, total_downloads DESC, c.name ASC ");
                } else {
                    qb.push("ORDER BY total_downloads DESC, c.name ASC ");
                }
            }
            CrateSearchSort::Downloads => {
                qb.push("ORDER BY total_downloads DESC, c.name ASC ");
            }
            CrateSearchSort::Recent => {
                qb.push(
                    "ORDER BY MAX(cv.published_at) DESC NULLS LAST, total_downloads DESC, c.name \
                     ASC ",
                );
            }
        }

        qb.push("LIMIT ");
        qb.push_bind(i64::from(limit));

        qb.build_query_as::<CrateSearchRow>()
            .fetch_all(&self.state.db)
            .await
    }

    pub(super) async fn handle_crate_search(
        &self,
        request: CrateSearchRequest,
    ) -> Result<Json<CrateSearchResponse>, String> {
        let query = normalize_optional(request.query);
        let category = normalize_optional(request.category);
        let keyword = normalize_optional(request.keyword);
        let limit = search_limit(request.limit);

        let sort = request
            .sort
            .unwrap_or_else(|| {
                if query.is_some() {
                    CrateSearchSort::Relevance
                } else {
                    CrateSearchSort::Downloads
                }
            });

        let rows = self
            .run_crate_search(
                query.as_deref(),
                category.as_deref(),
                keyword.as_deref(),
                sort,
                limit,
            )
            .await
            .map_err(|e| format!("crate.search query failed: {e}"))?;

        let mut freshness_checks_performed = 0_usize;
        let mut refresh_jobs_enqueued = 0_usize;

        for row in rows.iter().take(5) {
            let Some(latest_version) = row.latest_version.as_deref() else {
                continue;
            };

            let freshness = self
                .ensure_freshness_for_interaction(row.crate_id, &row.name, latest_version)
                .await?;

            if freshness.freshness_check_performed {
                freshness_checks_performed += 1;
            }
            if freshness.refresh_enqueued {
                refresh_jobs_enqueued += 1;
            }
        }

        let hits = rows
            .into_iter()
            .map(|row| CrateSearchHit {
                match_reasons: match_reasons(
                    &row,
                    query.as_deref(),
                    category.as_deref(),
                    keyword.as_deref(),
                ),
                name: row.name,
                description: row.description,
                repository_url: row.repository_url,
                docs_url: row.docs_url,
                homepage_url: row.homepage_url,
                categories: row.categories,
                keywords: row.keywords,
                total_downloads: row.total_downloads,
                latest_published_at: row.latest_published_at,
                dependent_crates: row.dependent_count,
                rank_score: row.relevance_score,
            })
            .collect::<Vec<_>>();

        Ok(Json(CrateSearchResponse {
            query,
            category,
            keyword,
            sort,
            limit,
            count: hits.len(),
            freshness_checks_performed,
            refresh_jobs_enqueued,
            freshness: vec![
                super::models::ResponseFreshnessSource {
                    source: "local_postgres_index".to_string(),
                    status: "fresh".to_string(),
                    checked_at: None,
                },
                super::models::ResponseFreshnessSource {
                    source: "crates.io".to_string(),
                    status: if freshness_checks_performed > 0 {
                        "probed".to_string()
                    } else {
                        "not_checked".to_string()
                    },
                    checked_at: None,
                },
            ],
            confidence: if hits.is_empty() { "low".to_string() } else { "high".to_string() },
            next_best_calls: if hits.is_empty() {
                vec!["index.sync_crates".to_string()]
            } else {
                vec![
                    "crate.intel".to_string(),
                    "crate.versions".to_string(),
                    "crate.graph".to_string(),
                ]
            },
            provenance: "local_postgres_index".to_string(),
            hits,
        }))
    }
}
