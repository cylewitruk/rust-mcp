//! docs.rs fixture payloads for e2e MCP tests.

use std::collections::HashMap;

use axum::extract::Path;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use rustdoc_types::{
    Abi, Crate as RustdocCrate, Function, FunctionHeader, FunctionSignature, Generics, Id, Item,
    ItemEnum, ItemKind, ItemSummary, Module, Target, Type as RustdocType,
    Visibility as RustdocVisibility,
};

use super::crates_io_fixtures::{
    SEEDED_ALT_CRATE_NAME, SEEDED_ALT_CRATE_VERSION, SEEDED_CRATE_NAME, SEEDED_CRATE_NEXT_VERSION,
    SEEDED_CRATE_VERSION,
};

pub const SEEDED_RUSTDOC_PATH: &str = "rustdoc-json/docs.rs/demo-crate-1.2.3.json";

fn build_rustdoc_fixture(
    crate_name: &str,
    crate_version: &str,
    function_name: &str,
    docs: &str,
) -> RustdocCrate {
    let mut index = HashMap::new();
    let mut paths = HashMap::new();

    index.insert(
        Id(0),
        Item {
            id: Id(0),
            crate_id: 0,
            name: Some(crate_name.to_string()),
            span: None,
            visibility: RustdocVisibility::Public,
            docs: None,
            links: HashMap::new(),
            attrs: Vec::new(),
            deprecation: None,
            inner: ItemEnum::Module(Module {
                is_crate: true,
                items: vec![Id(1)],
                is_stripped: false,
            }),
        },
    );
    paths.insert(
        Id(0),
        ItemSummary {
            crate_id: 0,
            path: vec![crate_name.to_string()],
            kind: ItemKind::Module,
        },
    );

    index.insert(
        Id(1),
        Item {
            id: Id(1),
            crate_id: 0,
            name: Some(function_name.to_string()),
            span: None,
            visibility: RustdocVisibility::Public,
            docs: Some(docs.to_string()),
            links: HashMap::new(),
            attrs: Vec::new(),
            deprecation: None,
            inner: ItemEnum::Function(Function {
                sig: FunctionSignature {
                    inputs: vec![("input".to_string(), RustdocType::Primitive("str".to_string()))],
                    output: Some(RustdocType::Primitive("bool".to_string())),
                    is_c_variadic: false,
                },
                generics: Generics {
                    params: vec![],
                    where_predicates: vec![],
                },
                header: FunctionHeader {
                    is_const: false,
                    is_unsafe: false,
                    is_async: false,
                    abi: Abi::Rust,
                },
                has_body: true,
            }),
        },
    );
    paths.insert(
        Id(1),
        ItemSummary {
            crate_id: 0,
            path: vec![crate_name.to_string(), function_name.to_string()],
            kind: ItemKind::Function,
        },
    );

    RustdocCrate {
        root: Id(0),
        crate_version: Some(crate_version.to_string()),
        includes_private: false,
        index,
        paths,
        external_crates: HashMap::new(),
        format_version: 57,
        target: Target {
            triple: "x86_64-unknown-linux-gnu".to_string(),
            target_features: vec![],
        },
    }
}

pub async fn mock_rustdoc_json_gzip(
    Path((crate_name, version)): Path<(String, String)>,
) -> impl IntoResponse {
    let rustdoc = match (crate_name.as_str(), version.as_str()) {
        (SEEDED_CRATE_NAME, SEEDED_CRATE_VERSION) => build_rustdoc_fixture(
            "demo_crate",
            SEEDED_CRATE_VERSION,
            "parse",
            "Parse fixture input",
        ),
        (SEEDED_CRATE_NAME, SEEDED_CRATE_NEXT_VERSION) => build_rustdoc_fixture(
            "demo_crate",
            SEEDED_CRATE_NEXT_VERSION,
            "parse_input",
            "Parse fixture input with richer behavior",
        ),
        (SEEDED_ALT_CRATE_NAME, SEEDED_ALT_CRATE_VERSION) => build_rustdoc_fixture(
            "demo_alt",
            SEEDED_ALT_CRATE_VERSION,
            "call_demo_parse",
            "Calls demo_crate::parse for fixture coverage",
        ),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };

    let bytes = serde_json::to_vec(&rustdoc).expect("failed to encode rustdoc fixture");
    (StatusCode::OK, [(header::CONTENT_TYPE, "application/json")], bytes).into_response()
}
