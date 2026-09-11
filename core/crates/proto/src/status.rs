//! gRPC status ↔ 错误的冻结映射（proto/aite/v1/edge.proto 头注释）。
use aite_contracts::{IngressError, PlatformError, SandboxError, SandboxErrorKind};
use tonic::{Code, Status};

/// status.message 形如 "<code>: <detail>"；拆不开时整段当 detail、code 用 gRPC code 名。
fn split_message(st: &Status) -> (String, String) {
    let msg = st.message();
    match msg.split_once(": ") {
        Some((code, detail)) if !code.is_empty() && !code.contains(' ') => {
            (code.to_string(), detail.to_string())
        }
        _ => (
            format!("grpc_{:?}", st.code()).to_lowercase(),
            msg.to_string(),
        ),
    }
}

pub fn is_retryable(code: Code) -> bool {
    matches!(
        code,
        Code::Unavailable | Code::DeadlineExceeded | Code::Aborted | Code::ResourceExhausted
    )
}

pub fn platform_error_from_status(st: &Status) -> PlatformError {
    let (code, detail) = split_message(st);
    let http_status = match st.code() {
        Code::InvalidArgument => Some(400),
        Code::PermissionDenied => Some(403),
        Code::NotFound => Some(404),
        _ => None,
    };
    PlatformError {
        code,
        message: detail,
        retryable: is_retryable(st.code()),
        http_status,
    }
}

pub fn sandbox_error_from_status(st: &Status) -> SandboxError {
    let (_, detail) = split_message(st);
    let kind = match st.code() {
        Code::Unavailable => SandboxErrorKind::Unavailable,
        Code::NotFound => {
            if st.message().starts_with("sandbox_not_found") {
                SandboxErrorKind::NotFound
            } else {
                SandboxErrorKind::FileNotFound
            }
        }
        Code::InvalidArgument => SandboxErrorKind::InvalidPath,
        Code::DeadlineExceeded => SandboxErrorKind::Timeout,
        _ => SandboxErrorKind::Internal,
    };
    SandboxError {
        kind,
        message: detail,
    }
}

/// core 侧 IngressService 的返回：非法事件 → INVALID_ARGUMENT（edge 不该重推），其余 → INTERNAL（edge 让平台重推）。
pub fn status_from_ingress_error(e: &IngressError) -> Status {
    match e {
        IngressError::Invalid(msg) => Status::invalid_argument(format!("invalid_event: {msg}")),
        other => Status::internal(format!("ingress_failed: {other}")),
    }
}
