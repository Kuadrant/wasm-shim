use crate::kuadrant::resolver::AttributeResolver;
use crate::proto::kuadrant::v1::{GetPluginConfigRequest, GetPluginConfigResponse};
use prost::Message;
use std::time::Duration;
use tracing::{debug, error};

pub const REMOTE_CONFIG_FETCH_TIMEOUT: Duration = Duration::from_secs(1);

/// Serializes an `envoy.config.core.v3.GrpcService{ envoy_grpc: EnvoyGrpc{
/// cluster_name } }` message. Envoy's `dispatch_grpc_call` ABI requires
/// `upstream_name` to be a serialized `GrpcService`, not a bare cluster name
/// string - see the comment at the call site in `fetch` for how this was
/// discovered. Written by hand rather than via prost-generated types to
/// avoid vendoring/compiling the full `envoy.config.core.v3` proto for two
/// string fields; uses `prost::encoding::encode_varint` for correct length
/// encoding regardless of cluster name length.
fn encode_grpc_service(cluster_name: &str) -> Vec<u8> {
    let mut envoy_grpc = Vec::new();
    envoy_grpc.push(0x0A); // field 1 (cluster_name), wire type 2 (length-delimited)
    prost::encoding::encode_varint(cluster_name.len() as u64, &mut envoy_grpc);
    envoy_grpc.extend_from_slice(cluster_name.as_bytes());

    let mut grpc_service = Vec::new();
    grpc_service.push(0x0A); // field 1 (envoy_grpc), wire type 2 (length-delimited)
    prost::encoding::encode_varint(envoy_grpc.len() as u64, &mut grpc_service);
    grpc_service.extend_from_slice(&envoy_grpc);

    grpc_service
}

#[derive(Default)]
enum State {
    #[default]
    Idle,
    Missing {
        gateway: String,
        cluster: String,
    },
    Pending {
        gateway: String,
        cluster: String,
        token: u32,
    },
}

/// Fetches the full wasm plugin configuration (ActionSets) for a gateway
/// from the operator's PluginConfigService, when the inline config received
/// via on_configure is a bootstrap-only stand-in for it (see
/// `PluginConfiguration::remote_config`). Mirrors `DescriptorManager`'s
/// dispatch/retry-via-tick pattern, but for a single (gateway, cluster)
/// target rather than a set of descriptor keys, and is owned exclusively by
/// `FilterRoot` rather than shared, so no interior mutability is needed.
#[derive(Default)]
pub struct RemoteConfigFetcher {
    state: State,
}

impl RemoteConfigFetcher {
    /// Marks a gateway's config as needing to be fetched from `cluster`.
    /// Safe to call repeatedly (e.g. on every on_configure) - overwrites
    /// any previous target/in-flight request.
    pub fn set_pending(&mut self, gateway: String, cluster: String) {
        self.state = State::Missing { gateway, cluster };
    }

    /// True once a fetch has been requested and hasn't yet been cleared via
    /// `clear` (i.e. still missing or in flight).
    pub fn is_active(&self) -> bool {
        !matches!(self.state, State::Idle)
    }

    /// Dispatches the fetch if one isn't already in flight. No-op if idle or
    /// already pending. Safe to call unconditionally on every tick.
    pub fn fetch(&mut self, ctx: &dyn AttributeResolver) {
        let (gateway, cluster) = match &self.state {
            State::Missing { gateway, cluster } => (gateway.clone(), cluster.clone()),
            _ => return,
        };

        let request = GetPluginConfigRequest {
            gateway: gateway.clone(),
        };
        let mut request_bytes = Vec::new();
        if let Err(e) = request.encode(&mut request_bytes) {
            error!("could not encode plugin config request: {}", e);
            return;
        }

        // Envoy's proxy_grpc_call ABI parses `upstream_name` as a serialized
        // envoy.config.core.v3.GrpcService (specifically its `envoy_grpc`
        // variant), not a bare cluster name string - confirmed empirically
        // against a live Envoy: passing the plain cluster name string
        // returns ParseFailure at dispatch time. DescriptorManager's
        // fetch_missing (descriptor_manager.rs) does the same bare-string
        // dispatch and would very likely hit the same failure on a real
        // cluster - never exercised end-to-end there, since the OOP
        // extension framework that would trigger it isn't yet in active
        // production use.
        let grpc_service_bytes = encode_grpc_service(&cluster);
        let cluster_arg = String::from_utf8(grpc_service_bytes).unwrap_or(cluster.clone());

        match ctx.dispatch_grpc_call(
            &cluster_arg,
            "kuadrant.v1.PluginConfigService",
            "GetPluginConfig",
            vec![],
            request_bytes,
            REMOTE_CONFIG_FETCH_TIMEOUT,
        ) {
            Ok(token) => {
                debug!(
                    "Dispatched plugin config fetch for gateway {} (token: {})",
                    gateway, token
                );
                self.state = State::Pending {
                    gateway,
                    cluster,
                    token,
                };
            }
            Err(e) => error!("could not dispatch plugin config fetch: {:?}", e),
        }
    }

    /// True if `token_id` is this fetcher's currently in-flight request.
    pub fn is_pending_token(&self, token_id: u32) -> bool {
        matches!(&self.state, State::Pending { token, .. } if *token == token_id)
    }

    /// Reverts to Missing (so the next tick retries) if `token_id` matches
    /// the in-flight request - used when the response indicates failure.
    pub fn reset_pending(&mut self, token_id: u32) {
        if let State::Pending {
            gateway,
            cluster,
            token,
        } = &self.state
        {
            if *token == token_id {
                self.state = State::Missing {
                    gateway: gateway.clone(),
                    cluster: cluster.clone(),
                };
            }
        }
    }

    /// Clears all state - call once the fetched config has been applied.
    pub fn clear(&mut self) {
        self.state = State::Idle;
    }

    pub fn decode_response(response_bytes: &[u8]) -> Result<GetPluginConfigResponse, String> {
        GetPluginConfigResponse::decode(response_bytes)
            .map_err(|e| format!("could not decode plugin config response: {}", e))
    }
}
