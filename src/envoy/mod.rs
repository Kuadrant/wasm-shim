// Generated protobuf modules
pub mod envoy {
    pub mod config {
        pub mod common {
            pub mod matcher {
                pub mod v3 {
                    include!("envoy.config.common.matcher.v3.rs");
                }
            }
        }
        pub mod core {
            pub mod v3 {
                include!("envoy.config.core.v3.rs");
            }
        }
        pub mod route {
            pub mod v3 {
                include!("envoy.config.route.v3.rs");
            }
        }
    }
    pub mod extensions {
        pub mod common {
            pub mod ratelimit {
                pub mod v3 {
                    include!("envoy.extensions.common.ratelimit.v3.rs");
                }
            }
        }
    }
    pub mod service {
        pub mod auth {
            pub mod v3 {
                include!("envoy.service.auth.v3.rs");
            }
        }
        pub mod ratelimit {
            pub mod v3 {
                include!("envoy.service.ratelimit.v3.rs");
            }
        }
    }
    pub mod r#type {
        pub mod matcher {
            pub mod v3 {
                include!("envoy.r#type.matcher.v3.rs");
            }
        }
        pub mod metadata {
            pub mod v3 {
                include!("envoy.r#type.metadata.v3.rs");
            }
        }
        pub mod tracing {
            pub mod v3 {
                include!("envoy.r#type.tracing.v3.rs");
            }
        }
        pub mod v3 {
            include!("envoy.r#type.v3.rs");
        }
    }
}

pub mod google {
    pub mod rpc {
        include!("google.rpc.rs");
    }
}

pub mod udpa {
    pub mod annotations {
        include!("udpa.annotations.rs");
    }
}

pub mod xds {
    pub mod annotations {
        pub mod v3 {
            include!("xds.annotations.v3.rs");
        }
    }
    pub mod core {
        pub mod v3 {
            include!("xds.core.v3.rs");
        }
    }
    pub mod r#type {
        pub mod matcher {
            pub mod v3 {
                include!("xds.r#type.matcher.v3.rs");
            }
        }
    }
}

pub mod validate {
    include!("validate.rs");
}

pub use envoy::config::core::v3::{
    Address, HeaderValue, HeaderValueOption, Metadata, SocketAddress,
};
pub use envoy::extensions::common::ratelimit::v3::{
    rate_limit_descriptor::Entry as RateLimitDescriptor_Entry, RateLimitDescriptor,
};
pub use envoy::service::auth::v3::{
    attribute_context::{
        HttpRequest as AttributeContext_HttpRequest, Peer as AttributeContext_Peer,
        Request as AttributeContext_Request,
    },
    AttributeContext, CheckRequest, CheckResponse, DeniedHttpResponse,
};
pub use envoy::service::ratelimit::v3::{
    rate_limit_response::Code as RateLimitResponse_Code, RateLimitRequest, RateLimitResponse,
};
pub use google::rpc::Status;

pub type StatusCode = i32;
