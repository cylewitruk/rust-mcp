//! Integration tests for `crate.*` MCP tools.

use rust_mcp_testing::fixtures::{seed_crate_version, seed_source_file, seed_symbol};
use serde_json::{Value, json};

use super::common;

async fn seed_type_intelligence_fixture(context: &common::SeededMcpContext) {
    let source_file_id = seed_source_file(
        &context.state.db,
        context
            .fixture
            .dependent
            .version_id,
        "src/error.rs",
        Some("rust"),
        "use std::fmt::{self, Display};\npub struct ParseError {\npub message: String,\n}\nimpl \
         ParseError {\npub fn new(message: String) -> ParseError { ParseError { message } \
         }\n}\nimpl Display for ParseError {\nfn fmt(&self, f: &mut fmt::Formatter<'_>) -> \
         fmt::Result {\nwrite!(f, \"parse error: {}\", self.message)\n}\n}\nimpl From<String> for \
         ParseError {\nfn from(value: String) -> ParseError { ParseError { message: value } \
         }\n}\npub fn parse() -> Result<(), ParseError> { \
         Err(ParseError::new(\"oops\".to_string())) }",
    )
    .await
    .expect("failed to seed type-intelligence source file");

    sqlx::query(
        "INSERT INTO crate_types (
            crate_version_id,
            source_file_id,
            type_name,
            kind,
            visibility,
            generic_params,
            fields,
            variants,
            start_line,
            end_line,
            index_source
         ) VALUES (
            $1, $2, 'ParseError', 'struct', 'public', $3::JSONB, $4::JSONB, $5::JSONB, $6, $7, \
         'fixture'
         )",
    )
    .bind(
        context
            .fixture
            .dependent
            .version_id,
    )
    .bind(source_file_id)
    .bind(json!([]))
    .bind(json!([{"name":"message","type":"String"}]))
    .bind(json!([]))
    .bind(2_i32)
    .bind(4_i32)
    .execute(&context.state.db)
    .await
    .expect("failed to seed crate_types row");

    sqlx::query(
        "INSERT INTO crate_impls (
            crate_version_id,
            source_file_id,
            type_name,
            type_name_display,
            trait_name,
            trait_name_display,
            impl_kind,
            methods,
            start_line,
            end_line,
            index_source
         ) VALUES (
            $1, $2, 'ParseError', 'ParseError', NULL, NULL, 'inherent', $3::JSONB, $4, $5, \
         'fixture'
         )",
    )
    .bind(
        context
            .fixture
            .dependent
            .version_id,
    )
    .bind(source_file_id)
    .bind(json!([{"name":"new","signature":"fn new(message: String) -> ParseError"}]))
    .bind(5_i32)
    .bind(7_i32)
    .execute(&context.state.db)
    .await
    .expect("failed to seed inherent impl row");

    sqlx::query(
        "INSERT INTO crate_impls (
            crate_version_id,
            source_file_id,
            type_name,
            type_name_display,
            trait_name,
            trait_name_display,
            impl_kind,
            methods,
            start_line,
            end_line,
            index_source
         ) VALUES (
            $1, $2, 'ParseError', 'ParseError', 'Display', 'std::fmt::Display', 'trait', \
         $3::JSONB, $4, $5, 'fixture'
         )",
    )
    .bind(
        context
            .fixture
            .dependent
            .version_id,
    )
    .bind(source_file_id)
    .bind(json!([{"name":"fmt","signature":"fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result"}]))
    .bind(8_i32)
    .bind(12_i32)
    .execute(&context.state.db)
    .await
    .expect("failed to seed Display impl row");

    sqlx::query(
        "INSERT INTO crate_impls (
            crate_version_id,
            source_file_id,
            type_name,
            type_name_display,
            trait_name,
            trait_name_display,
            impl_kind,
            methods,
            start_line,
            end_line,
            index_source
         ) VALUES (
            $1, $2, 'ParseError', 'ParseError', 'From', 'From<String>', 'trait', $3::JSONB, $4, \
         $5, 'fixture'
         )",
    )
    .bind(
        context
            .fixture
            .dependent
            .version_id,
    )
    .bind(source_file_id)
    .bind(json!([{"name":"from","signature":"fn from(value: String) -> ParseError"}]))
    .bind(13_i32)
    .bind(15_i32)
    .execute(&context.state.db)
    .await
    .expect("failed to seed From impl row");

    sqlx::query(
        "INSERT INTO crate_traits (
            crate_version_id,
            trait_name,
            is_auto,
            is_unsafe,
            is_dyn_compatible,
            supertraits,
            required_methods,
            provided_methods,
            associated_types,
            generics,
            index_source
         ) VALUES (
            $1, 'Display', FALSE, FALSE, TRUE, $2::JSONB, $3::JSONB, $4::JSONB, $5::JSONB, \
         $6::JSONB, 'fixture'
         )",
    )
    .bind(
        context
            .fixture
            .dependent
            .version_id,
    )
    .bind(json!([]))
    .bind(json!([{"name":"fmt","signature":"fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result"}]))
    .bind(json!([]))
    .bind(json!([]))
    .bind(json!([]))
    .execute(&context.state.db)
    .await
    .expect("failed to seed crate_traits row");

    sqlx::query(
        "INSERT INTO symbols (
            crate_version_id,
            source_file_id,
            name,
            kind,
            signature,
            visibility,
            start_line,
            end_line,
            index_source
         ) VALUES (
            $1, $2, 'parse', 'function', 'fn parse() -> Result<(), ParseError>', 'public', 16, 16, \
         'fixture'
         )",
    )
    .bind(
        context
            .fixture
            .dependent
            .version_id,
    )
    .bind(source_file_id)
    .execute(&context.state.db)
    .await
    .expect("failed to seed parse symbol with Result signature");
}

mod analysis;
mod compatibility;
mod discovery;
mod type_intel;
