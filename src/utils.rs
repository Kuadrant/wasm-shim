use crate::configuration::FailureMode;
use proxy_wasm::hostcalls::{resume_http_request, send_http_response};

#[derive(Debug, thiserror::Error)]
pub enum UtilsErr {
    #[error("failed to create string from utf8 data")]
    Utf8Fail(#[from] std::string::FromUtf8Error),
    #[error("problem while handing protobuf")]
    ProtobufErr(#[from] protobuf::error::ProtobufError),
    #[error("failed to get i64 from slice")]
    SliceToI64(#[from] std::array::TryFromSliceError),
    #[error("failed to convert from i64 to u32")]
    I64ToU32(#[from] std::num::TryFromIntError),
}

// Helper function to handle failure during processing.
pub fn request_process_failure(failure_mode: &FailureMode) {
    match failure_mode {
        FailureMode::Deny => {
            send_http_response(500, vec![], Some(b"Internal Server Error.\n")).unwrap()
        }
        FailureMode::Allow => resume_http_request().unwrap(),
    }
}
