// Data retrieved from the well-known attributes using Envoy v1.31-latest

#[cfg(test)]
pub mod request {
    pub mod method {
        pub const GET: &[u8] = &[71, 69, 84];
        pub const POST: &[u8] = &[80, 79, 83, 84];
    }
    pub mod scheme {
        pub const HTTP: &[u8] = &[104, 116, 116, 112];
        pub const HTTPS: &[u8] = &[104, 116, 116, 112, 115];
    }
    pub mod protocol {
        // 'HTTP/1.1'
        pub const HTTP_1_1: &[u8] = &[72, 84, 84, 80, 47, 49, 46, 49];
    }
    pub mod path {
        // '/admin'
        pub const ADMIN: &[u8] = &[47, 97, 100, 109, 105, 110];
        // '/admin/toy'
        pub const ADMIN_TOY: &[u8] = &[47, 97, 100, 109, 105, 110, 47, 116, 111, 121];
    }
    pub mod useragent {
        // 'curl/8.7.1'
        pub const CURL_8_7_1: &[u8] = &[99, 117, 114, 108, 47, 56, 46, 55, 46, 49];
    }
    // 'cars.toystore.com'
    pub const HOST: &[u8] = &[
        99, 97, 114, 115, 46, 116, 111, 121, 115, 116, 111, 114, 101, 46, 99, 111, 109,
    ];
    // '12d04ae3-6cfd-4e55-aad4-63555beb0bc5'
    pub const ID: &[u8] = &[
        49, 50, 100, 48, 52, 97, 101, 51, 45, 54, 99, 102, 100, 45, 52, 101, 53, 53, 45, 97, 97,
        100, 52, 45, 54, 51, 53, 53, 53, 98, 101, 98, 48, 98, 99, 53,
    ];
    pub const SIZE: &[u8] = &[0, 0, 0, 0, 0, 0, 0, 0];
    // 8 byte nanos from epoch: 1_730_987_538_880_438_000
    pub const TIME: &[u8] = &[240, 158, 152, 213, 254, 179, 5, 24];
}

pub mod source {
    // '127.0.0.1:45000'
    pub const ADDRESS: &[u8] = &[49, 50, 55, 46, 48, 46, 48, 46, 49, 58, 52, 53, 48, 48, 48];
    pub mod port {
        pub const P_45000: &[u8] = &[200, 175, 0, 0, 0, 0, 0, 0];
    }
}

pub mod destination {
    // '127.0.0.1:8000'
    pub const ADDRESS: &[u8] = &[49, 50, 55, 46, 48, 46, 48, 46, 49, 58, 56, 48, 48, 48];
    pub mod port {
        pub const P_8000: &[u8] = &[64, 31, 0, 0, 0, 0, 0, 0];
    }
}

pub mod connection {
    pub const ID: &[u8] = &[0, 0, 0, 0, 0, 0, 0, 0];
    pub const MTLS: &[u8] = &[0];
}

// Example CheckRequest
#[allow(dead_code)]
const CHECK_REQUEST: &str = r#"
attributes {
  source {
    address {
      socket_address {
        address: "127.0.0.1:45000"
        port_value: 45000
      }
    }
  }
  destination {
    address {
      socket_address {
        address: "127.0.0.1:8000"
        port_value: 8000
      }
    }
  }
  request {
    time {
      seconds: 1730987538
      nanos: 880438000
    }
    http {
      method: "GET"
      headers {
        key: ":authority"
        value: "abi_test_harness"
      }
      headers {
        key: ":method"
        value: "GET"
      }
      headers {
        key: ":path"
        value: "/default/request/headers/path"
      }
      path: "/admin/toy"
      host: "cars.toystore.com"
      scheme: "http"
      protocol: "HTTP/1.1"
    }
  }
  context_extensions {
    key: "host"
    value: "authconfig-A"
  }
  metadata_context {
  }
}
"#;

// Example Ratelimit Request
#[allow(dead_code)]
const RATELIMIT_REQUEST: &str = r#"
domain: "RLS-domain"
descriptors {
  entries {
    key: "admin"
    value: "1"
  }
}
hits_addend: 1
"#;
