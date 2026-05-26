use crate::proto::kuadrant::v1::{
    GetServiceDescriptorsRequest, GetServiceDescriptorsResponse, ServiceRef,
};
use prost::Message;
use prost_reflect::DescriptorPool;
use prost_types::FileDescriptorSet;
use proxy_wasm::traits::Context;
use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::Duration;
use tracing::{debug, error};

pub const DESCRIPTOR_FETCH_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct DescriptorKey {
    pub cluster: String,
    pub service: String,
}

impl DescriptorKey {
    pub fn new(cluster: String, service: String) -> Self {
        Self { cluster, service }
    }
}

#[derive(Debug)]
pub enum DescriptorError {
    NotAvailable { cluster: String, service: String },
}

impl std::fmt::Display for DescriptorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DescriptorError::NotAvailable { cluster, service } => {
                write!(
                    f,
                    "Descriptor not available for service {} at cluster {}",
                    service, cluster
                )
            }
        }
    }
}

impl std::error::Error for DescriptorError {}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

enum DescriptorState {
    Embedded(u64),
    Missing,
    Pending(u32),
    Resolved(u64),
}

pub struct DescriptorManager {
    pools: RefCell<HashMap<u64, Rc<DescriptorPool>>>,
    embedded: RefCell<HashMap<String, u64>>,
    descriptors: RefCell<HashMap<DescriptorKey, DescriptorState>>,
    descriptor_service: RefCell<Option<String>>,
}

impl Default for DescriptorManager {
    fn default() -> Self {
        let manager = Self {
            pools: Default::default(),
            embedded: Default::default(),
            descriptors: Default::default(),
            descriptor_service: Default::default(),
        };

        match embedded_descriptors::get_ratelimit_pool() {
            Ok((pool, bytes)) => {
                manager.register_embedded(
                    embedded_descriptors::RATELIMIT_SERVICE.to_string(),
                    bytes,
                    &pool,
                );
                manager.register_embedded(
                    embedded_descriptors::KUADRANT_RATELIMIT_SERVICE.to_string(),
                    bytes,
                    &pool,
                );
            }
            Err(e) => error!("failed to load embedded ratelimit descriptors: {}", e),
        }

        match embedded_descriptors::get_auth_pool() {
            Ok((pool, bytes)) => {
                manager.register_embedded(
                    embedded_descriptors::AUTH_SERVICE.to_string(),
                    bytes,
                    &pool,
                );
            }
            Err(e) => error!("failed to load embedded auth descriptors: {}", e),
        }

        manager
    }
}

impl DescriptorManager {
    pub fn set_descriptor_service(&self, service: &str) {
        let mut current = self.descriptor_service.borrow_mut();
        if current.as_ref().is_none_or(|s| s != service) {
            *current = Some(service.to_string());
        }
    }

    pub fn add_expected(&self, key: DescriptorKey) {
        let initial_state = self
            .embedded
            .borrow()
            .get(&key.service)
            .map(|&hash| DescriptorState::Embedded(hash))
            .unwrap_or(DescriptorState::Missing);

        self.descriptors
            .borrow_mut()
            .entry(key)
            .or_insert(initial_state);
    }

    pub fn has_expected(&self) -> bool {
        self.descriptors
            .borrow()
            .values()
            .any(|state| !matches!(state, DescriptorState::Embedded(_)))
    }

    pub fn tick_period(&self) -> Duration {
        DESCRIPTOR_FETCH_TIMEOUT * 2
    }

    pub fn get_pool(
        &self,
        cluster: &str,
        service: &str,
    ) -> Result<Rc<DescriptorPool>, DescriptorError> {
        let key = DescriptorKey::new(cluster.to_string(), service.to_string());

        self.descriptors
            .borrow()
            .get(&key)
            .and_then(|state| match state {
                DescriptorState::Embedded(hash) | DescriptorState::Resolved(hash) => {
                    self.pools.borrow().get(hash).map(Rc::clone)
                }
                _ => None,
            })
            .ok_or_else(|| DescriptorError::NotAvailable {
                cluster: cluster.to_string(),
                service: service.to_string(),
            })
    }

    fn register_embedded(&self, service: String, fds_bytes: &[u8], pool: &DescriptorPool) {
        let content_hash = hash_bytes(fds_bytes);

        self.pools
            .borrow_mut()
            .entry(content_hash)
            .or_insert_with(|| Rc::new(pool.clone()));

        self.embedded.borrow_mut().insert(service, content_hash);
    }

    #[cfg(test)]
    pub fn insert_pool(&self, key: DescriptorKey, pool: DescriptorPool) {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        let hash = hasher.finish();
        self.pools.borrow_mut().insert(hash, Rc::new(pool));
        self.descriptors
            .borrow_mut()
            .insert(key, DescriptorState::Resolved(hash));
    }

    fn insert_pool_from_bytes(&self, key: DescriptorKey, fds_bytes: &[u8], pool: DescriptorPool) {
        if matches!(
            self.descriptors.borrow().get(&key),
            Some(DescriptorState::Embedded(_))
        ) {
            return;
        }

        let content_hash = hash_bytes(fds_bytes);

        self.pools
            .borrow_mut()
            .entry(content_hash)
            .or_insert_with(|| Rc::new(pool));

        self.descriptors
            .borrow_mut()
            .insert(key, DescriptorState::Resolved(content_hash));
    }

    fn get_missing(&self) -> Vec<DescriptorKey> {
        self.descriptors
            .borrow()
            .iter()
            .filter_map(|(key, state)| {
                if matches!(state, DescriptorState::Missing) {
                    Some(key.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn fetch_missing(&self, ctx: &dyn Context) -> Result<(), String> {
        let missing = self.get_missing();

        if missing.is_empty() {
            return Ok(());
        }

        let descriptor_service = self
            .descriptor_service
            .borrow()
            .as_ref()
            .ok_or("descriptor service not configured")?
            .clone();

        debug!(
            "Fetching descriptors for {} missing services: {:?}",
            missing.len(),
            missing
        );

        let request = GetServiceDescriptorsRequest {
            services: missing
                .iter()
                .map(|key| ServiceRef {
                    cluster_name: key.cluster.clone(),
                    service: key.service.clone(),
                })
                .collect(),
        };

        let mut request_bytes = Vec::new();
        request
            .encode(&mut request_bytes)
            .map_err(|e| format!("could not encode descriptor request: {}", e))?;

        let token = ctx
            .dispatch_grpc_call(
                &descriptor_service,
                "kuadrant.v1.DescriptorService",
                "GetServiceDescriptors",
                vec![],
                Some(&request_bytes),
                DESCRIPTOR_FETCH_TIMEOUT,
            )
            .map_err(|status| format!("could not dispatch descriptor fetch: {:?}", status))?;

        debug!(
            "Dispatched descriptor fetch for {} services (token: {})",
            missing.len(),
            token
        );

        let mut descriptors = self.descriptors.borrow_mut();
        for key in missing {
            descriptors.insert(key, DescriptorState::Pending(token));
        }

        Ok(())
    }

    pub fn reset_pending(&self, token_id: u32) {
        self.descriptors
            .borrow_mut()
            .iter_mut()
            .for_each(|(_, state)| {
                if matches!(state, DescriptorState::Pending(t) if *t == token_id) {
                    *state = DescriptorState::Missing;
                }
            });
    }

    pub fn handle_response(&self, token_id: u32, response_bytes: Vec<u8>) -> Result<(), String> {
        let has_pending = self
            .descriptors
            .borrow()
            .values()
            .any(|state| matches!(state, DescriptorState::Pending(t) if *t == token_id));

        if !has_pending {
            return Err(format!(
                "Received descriptor response for unknown token {}",
                token_id
            ));
        }

        let response = GetServiceDescriptorsResponse::decode(response_bytes.as_slice())
            .map_err(|e| format!("could not decode descriptor response: {}", e))?;

        debug!(
            "Received {} service descriptors",
            response.descriptors.len()
        );

        let errors: Vec<_> = response
            .descriptors
            .into_iter()
            .filter_map(|descriptor| {
                let key = DescriptorKey::new(descriptor.cluster_name, descriptor.service);
                let fds_bytes = descriptor.file_descriptor_set;

                let result = FileDescriptorSet::decode(fds_bytes.as_slice())
                    .map_err(|e| format!("could not decode FileDescriptorSet for {:?}: {}", key, e))
                    .and_then(|fds| {
                        DescriptorPool::from_file_descriptor_set(fds).map_err(|e| {
                            format!("could not build DescriptorPool for {:?}: {}", key, e)
                        })
                    })
                    .and_then(|pool| {
                        if pool.get_service_by_name(&key.service).is_some() {
                            Ok(pool)
                        } else {
                            Err(format!(
                                "DescriptorPool for {:?} does not contain service {}",
                                key, key.service
                            ))
                        }
                    });

                match result {
                    Ok(pool) => {
                        debug!("Cached descriptor for {:?}", key);
                        self.insert_pool_from_bytes(key, &fds_bytes, pool);
                        None
                    }
                    Err(e) => {
                        error!("{}", e);
                        Some(e)
                    }
                }
            })
            .collect();

        if !errors.is_empty() {
            return Err(format!("Failed to process {} descriptor(s)", errors.len()));
        }

        Ok(())
    }
}

mod embedded_descriptors {
    use prost::Message;
    use prost_reflect::DescriptorPool;
    use prost_types::FileDescriptorSet;

    const RATELIMIT_DESCRIPTORS: &[u8] =
        include_bytes!(concat!(env!("OUT_DIR"), "/ratelimit_descriptors.bin"));
    const AUTH_DESCRIPTORS: &[u8] =
        include_bytes!(concat!(env!("OUT_DIR"), "/auth_descriptors.bin"));

    pub const RATELIMIT_SERVICE: &str = "envoy.service.ratelimit.v3.RateLimitService";
    pub const KUADRANT_RATELIMIT_SERVICE: &str = "kuadrant.service.ratelimit.v1.RateLimitService";
    pub const AUTH_SERVICE: &str = "envoy.service.auth.v3.Authorization";

    pub fn get_ratelimit_pool() -> Result<(DescriptorPool, &'static [u8]), String> {
        let fds = FileDescriptorSet::decode(RATELIMIT_DESCRIPTORS)
            .map_err(|e| format!("Failed to decode ratelimit descriptors: {}", e))?;

        let pool = DescriptorPool::from_file_descriptor_set(fds)
            .map_err(|e| format!("Failed to create ratelimit descriptor pool: {}", e))?;

        Ok((pool, RATELIMIT_DESCRIPTORS))
    }

    pub fn get_auth_pool() -> Result<(DescriptorPool, &'static [u8]), String> {
        let fds = FileDescriptorSet::decode(AUTH_DESCRIPTORS)
            .map_err(|e| format!("Failed to decode auth descriptors: {}", e))?;

        let pool = DescriptorPool::from_file_descriptor_set(fds)
            .map_err(|e| format!("Failed to create auth descriptor pool: {}", e))?;

        Ok((pool, AUTH_DESCRIPTORS))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost_types::{
        field_descriptor_proto, DescriptorProto, FieldDescriptorProto, FileDescriptorProto,
        FileDescriptorSet, MethodDescriptorProto, ServiceDescriptorProto,
    };

    fn create_test_descriptor_pool() -> DescriptorPool {
        let file_descriptor = FileDescriptorProto {
            name: Some("test.proto".to_string()),
            package: Some("test".to_string()),
            message_type: vec![
                DescriptorProto {
                    name: Some("Request".to_string()),
                    field: vec![FieldDescriptorProto {
                        name: Some("id".to_string()),
                        number: Some(1),
                        r#type: Some(field_descriptor_proto::Type::String.into()),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                DescriptorProto {
                    name: Some("Response".to_string()),
                    field: vec![FieldDescriptorProto {
                        name: Some("result".to_string()),
                        number: Some(1),
                        r#type: Some(field_descriptor_proto::Type::String.into()),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            ],
            service: vec![ServiceDescriptorProto {
                name: Some("TestService".to_string()),
                method: vec![MethodDescriptorProto {
                    name: Some("TestMethod".to_string()),
                    input_type: Some(".test.Request".to_string()),
                    output_type: Some(".test.Response".to_string()),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        let fds = FileDescriptorSet {
            file: vec![file_descriptor],
        };

        DescriptorPool::from_file_descriptor_set(fds).expect("Failed to create descriptor pool")
    }

    #[test]
    fn test_get_pool_returns_error_when_not_available() {
        let manager = DescriptorManager::default();

        let result = manager.get_pool("test-cluster", "test.Service");
        assert!(result.is_err());

        assert!(
            matches!(result,  Err(DescriptorError::NotAvailable { cluster, service }) if cluster == "test-cluster" && service == "test.Service")
        );
    }

    #[test]
    fn test_insert_and_get_pool() {
        let manager = DescriptorManager::default();
        let pool = create_test_descriptor_pool();

        let key = DescriptorKey::new("test-cluster".to_string(), "test.TestService".to_string());
        manager.insert_pool(key, pool);

        let result = manager.get_pool("test-cluster", "test.TestService");
        assert!(result.is_ok());

        let retrieved_pool = result.unwrap();
        let service = retrieved_pool.get_service_by_name("test.TestService");
        assert!(service.is_some());
    }

    #[test]
    fn test_deduplication_same_descriptor_bytes() {
        let manager = DescriptorManager::default();
        let initial_pool_count = manager.pools.borrow().len();

        let file_descriptor = FileDescriptorProto {
            name: Some("test.proto".to_string()),
            package: Some("test".to_string()),
            message_type: vec![DescriptorProto {
                name: Some("TestMessage".to_string()),
                ..Default::default()
            }],
            service: vec![ServiceDescriptorProto {
                name: Some("TestService".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };

        let fds = FileDescriptorSet {
            file: vec![file_descriptor],
        };

        let mut fds_bytes = Vec::new();
        fds.encode(&mut fds_bytes).unwrap();

        let pool1 = DescriptorPool::from_file_descriptor_set(
            FileDescriptorSet::decode(fds_bytes.as_slice()).unwrap(),
        )
        .unwrap();
        let pool2 = DescriptorPool::from_file_descriptor_set(
            FileDescriptorSet::decode(fds_bytes.as_slice()).unwrap(),
        )
        .unwrap();

        let key1 = DescriptorKey::new("cluster-a".to_string(), "test.TestService".to_string());
        let key2 = DescriptorKey::new("cluster-b".to_string(), "test.TestService".to_string());

        manager.insert_pool_from_bytes(key1, &fds_bytes, pool1);
        manager.insert_pool_from_bytes(key2, &fds_bytes, pool2);

        let result1 = manager.get_pool("cluster-a", "test.TestService").unwrap();
        let result2 = manager.get_pool("cluster-b", "test.TestService").unwrap();

        assert!(Rc::ptr_eq(&result1, &result2));

        assert_eq!(manager.pools.borrow().len(), initial_pool_count + 1);
    }

    #[test]
    fn test_embedded_descriptors() {
        let manager = DescriptorManager::default();

        let file_descriptor = FileDescriptorProto {
            name: Some("test.proto".to_string()),
            package: Some("test".to_string()),
            service: vec![ServiceDescriptorProto {
                name: Some("TestService".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };

        let fds = FileDescriptorSet {
            file: vec![file_descriptor],
        };

        let mut fds_bytes = Vec::new();
        fds.encode(&mut fds_bytes).unwrap();

        let pool = DescriptorPool::from_file_descriptor_set(
            FileDescriptorSet::decode(fds_bytes.as_slice()).unwrap(),
        )
        .unwrap();

        manager.register_embedded("test.TestService".to_string(), &fds_bytes, &pool);

        let key = DescriptorKey::new("any-cluster".to_string(), "test.TestService".to_string());
        manager.add_expected(key);

        let result = manager.get_pool("any-cluster", "test.TestService");
        assert!(result.is_ok());
    }

    #[test]
    fn test_embedded_not_marked_missing() {
        let manager = DescriptorManager::default();

        let file_descriptor = FileDescriptorProto {
            name: Some("embedded.proto".to_string()),
            package: Some("test".to_string()),
            service: vec![ServiceDescriptorProto {
                name: Some("TestService".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };

        let fds = FileDescriptorSet {
            file: vec![file_descriptor],
        };

        let mut fds_bytes = Vec::new();
        fds.encode(&mut fds_bytes).unwrap();

        let pool = DescriptorPool::from_file_descriptor_set(
            FileDescriptorSet::decode(fds_bytes.as_slice()).unwrap(),
        )
        .unwrap();

        manager.register_embedded("test.TestService".to_string(), &fds_bytes, &pool);

        let key = DescriptorKey::new("test-cluster".to_string(), "test.TestService".to_string());
        manager.add_expected(key.clone());

        assert!(matches!(
            manager.descriptors.borrow().get(&key),
            Some(DescriptorState::Embedded(_))
        ),);

        assert!(manager.get_missing().is_empty());
        assert!(!manager.has_expected());
    }

    #[test]
    fn test_embedded_resolves_for_any_cluster() {
        let manager = DescriptorManager::default();

        let embedded_fd = FileDescriptorProto {
            name: Some("embedded.proto".to_string()),
            package: Some("test".to_string()),
            service: vec![ServiceDescriptorProto {
                name: Some("TestService".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };

        let embedded_fds = FileDescriptorSet {
            file: vec![embedded_fd],
        };

        let mut embedded_bytes = Vec::new();
        embedded_fds.encode(&mut embedded_bytes).unwrap();

        let embedded_pool = DescriptorPool::from_file_descriptor_set(
            FileDescriptorSet::decode(embedded_bytes.as_slice()).unwrap(),
        )
        .unwrap();

        manager.register_embedded(
            "test.TestService".to_string(),
            &embedded_bytes,
            &embedded_pool,
        );

        let key_a = DescriptorKey::new("cluster-a".to_string(), "test.TestService".to_string());
        let key_b = DescriptorKey::new("cluster-b".to_string(), "test.TestService".to_string());
        manager.add_expected(key_a);
        manager.add_expected(key_b);

        let result_a = manager.get_pool("cluster-a", "test.TestService").unwrap();
        let result_b = manager.get_pool("cluster-b", "test.TestService").unwrap();

        assert!(Rc::ptr_eq(&result_a, &result_b));
        assert_eq!(
            result_a.services().next().unwrap().parent_file().name(),
            "embedded.proto"
        );
    }

    #[test]
    fn test_embedded_entries_share_pool_across_clusters() {
        let manager = DescriptorManager::default();
        let initial_pool_count = manager.pools.borrow().len();

        let file_descriptor = FileDescriptorProto {
            name: Some("test.proto".to_string()),
            package: Some("test".to_string()),
            service: vec![ServiceDescriptorProto {
                name: Some("TestService".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };

        let fds = FileDescriptorSet {
            file: vec![file_descriptor],
        };

        let mut fds_bytes = Vec::new();
        fds.encode(&mut fds_bytes).unwrap();

        let pool = DescriptorPool::from_file_descriptor_set(
            FileDescriptorSet::decode(fds_bytes.as_slice()).unwrap(),
        )
        .unwrap();

        manager.register_embedded("test.TestService".to_string(), &fds_bytes, &pool);

        let key_a = DescriptorKey::new("cluster-a".to_string(), "test.TestService".to_string());
        let key_b = DescriptorKey::new("cluster-b".to_string(), "test.TestService".to_string());
        manager.add_expected(key_a);
        manager.add_expected(key_b);

        let result_a = manager.get_pool("cluster-a", "test.TestService").unwrap();
        let result_b = manager.get_pool("cluster-b", "test.TestService").unwrap();

        assert!(Rc::ptr_eq(&result_a, &result_b));
        assert_eq!(manager.pools.borrow().len(), initial_pool_count + 1);
    }

    #[test]
    fn test_add_expected_embedded_service() {
        let manager = DescriptorManager::default();

        let key = DescriptorKey::new(
            "limitador-cluster".to_string(),
            "envoy.service.ratelimit.v3.RateLimitService".to_string(),
        );
        manager.add_expected(key.clone());

        assert!(matches!(
            manager.descriptors.borrow().get(&key),
            Some(DescriptorState::Embedded(_))
        ));
        assert!(!manager.has_expected());
        assert!(manager.get_missing().is_empty());

        let pool = manager
            .get_pool(
                "limitador-cluster",
                "envoy.service.ratelimit.v3.RateLimitService",
            )
            .expect("Should have embedded rate limit pool");
        assert!(Rc::strong_count(&pool) >= 1);
    }

    #[test]
    fn test_add_expected_non_embedded_marks_missing() {
        let manager = DescriptorManager::default();

        let key = DescriptorKey::new("custom-cluster".to_string(), "custom.Service".to_string());
        manager.add_expected(key.clone());

        assert!(matches!(
            manager.descriptors.borrow().get(&key),
            Some(DescriptorState::Missing)
        ));
        assert!(manager.has_expected());

        let missing = manager.get_missing();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0], key);
    }
}
