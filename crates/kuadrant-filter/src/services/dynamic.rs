use std::rc::Rc;
use std::time::Duration;

use cel::Value;
use prost::Message;
use prost_reflect::DynamicMessage;
use tracing::debug;

use super::{Service, ServiceError};
use crate::configuration::FailureMode;
use crate::descriptor_manager::{DescriptorKey, DescriptorManager};
use crate::kuadrant::ReqRespCtx;

pub mod converters;

use converters::MessageConverter;

pub struct DynamicService {
    upstream_name: String,
    service_name: String,
    method: String,
    timeout: Duration,
    failure_mode: FailureMode,
    descriptor_manager: Rc<DescriptorManager>,
}

impl DynamicService {
    pub fn new(
        endpoint: String,
        grpc_service: String,
        grpc_method: String,
        timeout: Duration,
        failure_mode: FailureMode,
        descriptor_manager: Rc<DescriptorManager>,
    ) -> Self {
        descriptor_manager.add_expected(DescriptorKey::new(endpoint.clone(), grpc_service.clone()));

        Self {
            upstream_name: endpoint,
            service_name: grpc_service,
            method: grpc_method,
            timeout,
            failure_mode,
            descriptor_manager,
        }
    }

    pub fn failure_mode(&self) -> FailureMode {
        self.failure_mode
    }

    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn dispatch_value(
        &self,
        ctx: &mut ReqRespCtx,
        cel_value: &Value,
    ) -> Result<u32, ServiceError> {
        let input_descriptor = self.input_descriptor()?;

        debug!("Converting CEL value to protobuf message");
        let request_message =
            MessageConverter::cel_to_dynamic_message(cel_value, &input_descriptor).map_err(
                |e| ServiceError::Dispatch(format!("Failed to convert CEL to message: {}", e)),
            )?;

        let mut message_bytes = Vec::new();
        request_message
            .encode(&mut message_bytes)
            .map_err(|e| ServiceError::Dispatch(format!("Failed to encode message: {}", e)))?;

        self.dispatch(
            ctx,
            &self.upstream_name,
            &self.service_name,
            &self.method,
            message_bytes,
            self.timeout,
        )
    }

    fn method_descriptor(&self) -> Result<prost_reflect::MethodDescriptor, ServiceError> {
        let pool = self
            .descriptor_manager
            .get_pool(&self.upstream_name, &self.service_name)
            .map_err(|e| ServiceError::Dispatch(e.to_string()))?;

        let service_descriptor = pool
            .get_service_by_name(&self.service_name)
            .ok_or_else(|| {
                ServiceError::Dispatch(format!(
                    "Service '{}' not found in descriptor pool",
                    self.service_name
                ))
            })?;
        let method = service_descriptor
            .methods()
            .find(|m| m.name() == self.method)
            .ok_or_else(|| {
                ServiceError::Dispatch(format!(
                    "Method '{}' not found in service '{}'",
                    self.method, self.service_name
                ))
            })?;
        Ok(method)
    }

    pub fn input_descriptor(&self) -> Result<prost_reflect::MessageDescriptor, ServiceError> {
        Ok(self.method_descriptor()?.input())
    }

    pub fn output_descriptor(&self) -> Result<prost_reflect::MessageDescriptor, ServiceError> {
        Ok(self.method_descriptor()?.output())
    }

    pub fn get_response_cel_value(
        &self,
        ctx: &mut ReqRespCtx,
        response_size: usize,
    ) -> Result<Value, ServiceError> {
        let response = self.get_response(ctx, response_size)?;
        MessageConverter::dynamic_message_to_cel(&response)
            .map_err(|e| ServiceError::Decode(format!("Failed to convert message to CEL: {}", e)))
    }
}

impl Service for DynamicService {
    type Response = DynamicMessage;

    fn parse_message(&self, message: Vec<u8>) -> Result<Self::Response, ServiceError> {
        let pool = self
            .descriptor_manager
            .get_pool(&self.upstream_name, &self.service_name)
            .map_err(|e| ServiceError::Decode(e.to_string()))?;

        let service_descriptor = pool
            .get_service_by_name(&self.service_name)
            .ok_or_else(|| {
                ServiceError::Decode(format!(
                    "Service '{}' not found in descriptor pool",
                    self.service_name
                ))
            })?;
        let method_descriptor = service_descriptor
            .methods()
            .find(|m| m.name() == self.method)
            .ok_or_else(|| {
                ServiceError::Decode(format!(
                    "Method '{}' not found in service '{}'",
                    self.method, self.service_name
                ))
            })?;
        let output_descriptor = method_descriptor.output();
        let response = DynamicMessage::decode(output_descriptor, message.as_slice())
            .map_err(|e| ServiceError::Decode(format!("Failed to decode response: {}", e)))?;

        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::descriptor_manager::{DescriptorKey, DescriptorManager};
    use crate::services::DescriptorConverter;
    use cel::{Context, Env, Program};
    use prost_reflect::DescriptorPool;
    use prost_types::{
        field_descriptor_proto, DescriptorProto, FieldDescriptorProto, FileDescriptorProto,
        FileDescriptorSet, MethodDescriptorProto, ServiceDescriptorProto,
    };

    fn create_test_descriptor_manager() -> Rc<DescriptorManager> {
        let file_descriptor = FileDescriptorProto {
            name: Some("test.proto".to_string()),
            package: Some("test".to_string()),
            message_type: vec![
                DescriptorProto {
                    name: Some("TestRequest".to_string()),
                    field: vec![FieldDescriptorProto {
                        name: Some("message".to_string()),
                        number: Some(1),
                        r#type: Some(field_descriptor_proto::Type::String.into()),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                DescriptorProto {
                    name: Some("TestResponse".to_string()),
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
                    input_type: Some(".test.TestRequest".to_string()),
                    output_type: Some(".test.TestResponse".to_string()),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        let fds = FileDescriptorSet {
            file: vec![file_descriptor],
        };

        let pool = DescriptorPool::from_file_descriptor_set(fds)
            .expect("Failed to create descriptor pool");

        let manager = Rc::new(DescriptorManager::default());
        let key = DescriptorKey::new("test-cluster".to_string(), "test.TestService".to_string());
        manager.insert_pool(key, pool);

        manager
    }

    #[test]
    fn test_dynamic_service_cel_message_building() {
        let manager = create_test_descriptor_manager();
        let service = DynamicService::new(
            "test-cluster".to_string(),
            "test.TestService".to_string(),
            "TestMethod".to_string(),
            Duration::from_secs(1),
            FailureMode::Deny,
            manager.clone(),
        );

        let cel_expression = r#"test.TestRequest { message: "hello" }"#;

        let pool = manager
            .get_pool("test-cluster", "test.TestService")
            .expect("Pool not found");
        let service_desc = pool
            .get_service_by_name(&service.service_name)
            .expect("Service not found");
        let method_desc = service_desc
            .methods()
            .find(|m| m.name() == service.method)
            .expect("Method not found");
        let input_desc = method_desc.input();

        let mut env = Env::stdlib();
        for def in DescriptorConverter::collect_struct_defs(&input_desc)
            .expect("Failed to collect struct defs")
        {
            env.add_struct(def);
        }

        let cel_ctx = Context::with_env(Arc::new(env));
        let program = Program::compile(cel_expression).expect("Failed to compile");
        let cel_value = program.execute(&cel_ctx).expect("Failed to execute");

        let message = MessageConverter::cel_to_dynamic_message(&cel_value, &input_desc)
            .expect("Failed to convert");

        let mut bytes = Vec::new();
        message.encode(&mut bytes).expect("Failed to encode");
        assert!(!bytes.is_empty());

        let field_value = message
            .get_field_by_name("message")
            .expect("message field not found");
        assert_eq!(
            field_value.as_ref(),
            &prost_reflect::Value::String("hello".to_string())
        );
    }

    #[test]
    fn test_parse_message_with_valid_response() {
        let manager = create_test_descriptor_manager();
        let service = DynamicService::new(
            "test-cluster".to_string(),
            "test.TestService".to_string(),
            "TestMethod".to_string(),
            Duration::from_secs(1),
            FailureMode::Deny,
            manager.clone(),
        );

        let pool = manager
            .get_pool("test-cluster", "test.TestService")
            .expect("Pool not found");
        let service_desc = pool
            .get_service_by_name(&service.service_name)
            .expect("Service not found");
        let method_desc = service_desc
            .methods()
            .find(|m| m.name() == service.method)
            .expect("Method not found");
        let output_desc = method_desc.output();

        let response_json = r#"{ "result": "success" }"#;
        let mut deserializer = serde_json::Deserializer::from_str(response_json);
        let dynamic_response = DynamicMessage::deserialize(output_desc, &mut deserializer)
            .expect("Failed to deserialize response");

        let mut response_bytes = Vec::new();
        dynamic_response
            .encode(&mut response_bytes)
            .expect("Failed to encode");

        let parsed = service.parse_message(response_bytes);
        assert!(parsed.is_ok());
    }
}
