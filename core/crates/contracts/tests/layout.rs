//! R0 必须落成的目录布局（对应旧 tests/contracts/test_layout.py）。缺一个就是骨架没搭完。
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

#[test]
fn frozen_paths_exist() {
    let root = repo_root();
    let must = [
        ".contracts.lock",
        "docs/dev-spec-2026-09-11-rustgo.md",
        "proto/aite/v1/capabilities.proto",
        "proto/aite/v1/events.proto",
        "proto/aite/v1/outbound.proto",
        "proto/aite/v1/sandbox.proto",
        "proto/aite/v1/edge.proto",
        "edge/go.mod",
        "edge/gen/aitepb/edge.pb.go",
        "edge/gen/aitepb/edge_grpc.pb.go",
        "edge/cmd/aite-edge/main.go",
        "edge/internal/feishu/platform.go",
        "edge/internal/sandbox/docker.go",
        "edge/internal/server/ports.go",
        "core/Cargo.toml",
        "core/rust-toolchain.toml",
        "core/crates/contracts/Cargo.toml",
        "core/crates/proto/Cargo.toml",
        "core/crates/store/Cargo.toml",
        "core/crates/evidence/Cargo.toml",
        "core/crates/gateway/Cargo.toml",
        "core/crates/worker/Cargo.toml",
        "core/crates/worker/prompts/platform.md",
        "core/crates/control/Cargo.toml",
        "core/crates/models/Cargo.toml",
        "core/crates/edge-client/Cargo.toml",
        "core/crates/testing/Cargo.toml",
        "core/crates/evals/Cargo.toml",
        "core/crates/app/Cargo.toml",
        "config/aite.example.yaml",
        "evals/p0/01_simple_qa.yaml",
        "docker/sandbox/Dockerfile",
    ];
    let missing: Vec<_> = must.iter().filter(|p| !root.join(p).exists()).collect();
    assert!(missing.is_empty(), "缺：{missing:?}");
}
