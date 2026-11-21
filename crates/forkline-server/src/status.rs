//! Mapping plan outcomes to protocol status codes.

use forkline_core::ErrorKind;

pub(crate) fn http_status(kind: ErrorKind) -> http::StatusCode {
    match kind {
        ErrorKind::BadRequest => http::StatusCode::BAD_REQUEST,
        ErrorKind::Deadline => http::StatusCode::GATEWAY_TIMEOUT,
        ErrorKind::Cancelled => http::StatusCode::SERVICE_UNAVAILABLE,
        ErrorKind::Module | ErrorKind::Panic | ErrorKind::MergeConflict | ErrorKind::Internal => {
            http::StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

pub(crate) fn grpc_code(kind: ErrorKind) -> tonic::Code {
    match kind {
        ErrorKind::BadRequest => tonic::Code::InvalidArgument,
        ErrorKind::Deadline => tonic::Code::DeadlineExceeded,
        ErrorKind::Cancelled => tonic::Code::Cancelled,
        ErrorKind::Module | ErrorKind::Panic | ErrorKind::MergeConflict | ErrorKind::Internal => tonic::Code::Internal,
    }
}
