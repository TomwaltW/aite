//! aite-proto：core ↔ edge 进程边界的 Rust 侧。
//!
//! - `pb`：prost/tonic 生成的消息与服务（与 edge/gen/aitepb 同源，都来自 proto/aite/v1）
//! - `convert`：domain（aite-contracts）↔ pb 的互转；pb 的 Unspecified 枚举 / 缺失必填字段 → ConvertError
//! - `status`：gRPC status ↔ PlatformError / SandboxError / IngressError 的冻结映射（edge.proto 头注释）
//!
//! owner: R0。各轨只调用不修改；需要新转换 → 停下报告。

pub mod pb {
    tonic::include_proto!("aite.v1");
}

pub mod convert;
pub mod status;

pub use convert::ConvertError;
