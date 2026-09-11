//! 编译期从 ../../../proto/aite/v1/*.proto 生成 tonic 代码。需要 PATH 上有 protoc（brew install protobuf）。
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../proto");
    let files = ["capabilities", "events", "outbound", "sandbox", "edge"]
        .iter()
        .map(|f| root.join(format!("aite/v1/{f}.proto")))
        .collect::<Vec<_>>();
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
    }
    tonic_prost_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&files, &[root])?;
    Ok(())
}
