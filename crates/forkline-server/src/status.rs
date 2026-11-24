//! Mapping plan outcomes to protocol status codes.

use forkline_core::ErrorKind;

pub(crate) fn http_status(kind: ErrorKind) -> http::StatusCode {
    match kind {
        ErrorKind::BadRequest => http::StatusCode::BAD_REQUEST,
        ErrorKind::Deadline => http::StatusCode::GATEWAY_TIMEOUT,
        ErrorKind::Cancelled | ErrorKind::Overloaded => http::StatusCode::SERVICE_UNAVAILABLE,
        _ => http::StatusCode::INTERNAL_SERVER_ERROR,
    }
}

pub(crate) fn grpc_code(kind: ErrorKind) -> tonic::Code {
    match kind {
        ErrorKind::BadRequest => tonic::Code::InvalidArgument,
        ErrorKind::Deadline => tonic::Code::DeadlineExceeded,
        ErrorKind::Cancelled => tonic::Code::Cancelled,
        ErrorKind::Overloaded => tonic::Code::ResourceExhausted,
        _ => tonic::Code::Internal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_has_a_status() {
        let table = [
            (ErrorKind::BadRequest, 400, tonic::Code::InvalidArgument),
            (ErrorKind::Module, 500, tonic::Code::Internal),
            (ErrorKind::Deadline, 504, tonic::Code::DeadlineExceeded),
            (ErrorKind::Cancelled, 503, tonic::Code::Cancelled),
            (ErrorKind::Panic, 500, tonic::Code::Internal),
            (ErrorKind::MergeConflict, 500, tonic::Code::Internal),
            (ErrorKind::Internal, 500, tonic::Code::Internal),
            (ErrorKind::Overloaded, 503, tonic::Code::ResourceExhausted),
        ];
        for (kind, http, grpc) in table {
            assert_eq!(http_status(kind).as_u16(), http, "{kind}");
            assert_eq!(grpc_code(kind), grpc, "{kind}");
        }
    }
}
