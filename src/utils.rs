use crate::envoy::{
    Address, AttributeContext_Peer, AttributeContext_Request, HeaderMatcher,
    HeaderMatcher_specifier, RLA_action_specifier, RateLimitDescriptor, RateLimitDescriptor_Entry,
    SocketAddress, SocketAddress_Protocol, StringMatcher_pattern,
};
use log::warn;
use proxy_wasm::hostcalls::{resume_http_request, send_http_response};
use proxy_wasm::traits::HttpContext;
use std::collections::HashMap;

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
pub fn request_process_failure(failure_mode_deny: bool) {
    if failure_mode_deny {
        send_http_response(500, vec![], Some(b"Internal Server Error.\n")).unwrap();
    }
    resume_http_request().unwrap();
}

fn read_le_i64(input: &[u8]) -> Result<i64, UtilsErr> {
    let clone = input.to_owned();
    let (uint_bytes, _) = clone.split_at(std::mem::size_of::<i64>());
    let array_form: [u8; 8] = uint_bytes.try_into()?;
    Ok(i64::from_le_bytes(array_form))
}

pub fn set_attribute_peer(
    filter: &dyn HttpContext,
    peer: &mut AttributeContext_Peer,
    service: String,
    local: bool,
) -> Result<(), UtilsErr> {
    if local {
        let local_address_bytes = filter
            .get_property(vec!["destination", "address"])
            .unwrap_or_else(|| {
                warn!("local address not found");
                vec![]
            });
        let local_address = String::from_utf8(local_address_bytes)?;

        let local_port_bytes = filter
            .get_property(vec!["destination", "port"])
            .unwrap_or_else(|| {
                warn!("local port not found");
                vec![]
            });
        let local_port: u32 = read_le_i64(&local_port_bytes)?.try_into()?; // ER

        set_peer_address(peer, local_address, local_port);
    } else {
        let remote_address_bytes = filter
            .get_property(vec!["source", "address"])
            .unwrap_or_else(|| {
                warn!("rempte address not found");
                vec![]
            });
        let remote_address = String::from_utf8(remote_address_bytes)?;

        let remote_port_bytes = filter
            .get_property(vec!["source", "port"])
            .unwrap_or_else(|| {
                warn!("remote port not found");
                vec![]
            });
        let remote_port: u32 = read_le_i64(&remote_port_bytes)?.try_into()?; // ER

        set_peer_address(peer, remote_address, remote_port);
    }

    let mtls_bytes = filter.get_property(vec!["connection", "mtls"]).unwrap();
    let mtls = mtls_bytes[0] != 0;
    if mtls {
        if local {
            let uri_sans_bytes =
                filter.get_property(vec!["connection", "uri_san_local_certificate"]);
            if uri_sans_bytes.is_none() {
                let dns_sans_bytes =
                    filter.get_property(vec!["connection", "dns_san_local_certificate"]);
                if dns_sans_bytes.is_none() {
                    let sub_local_bytes = filter
                        .get_property(vec!["connection", "subject_local_certificate"])
                        .unwrap();
                    let sub_local = String::from_utf8(sub_local_bytes)?;
                    peer.set_principal(sub_local);
                } else {
                    let dns_sans = String::from_utf8(dns_sans_bytes.unwrap())?;
                    peer.set_principal(dns_sans);
                }
            } else {
                let uri_sans = String::from_utf8(uri_sans_bytes.unwrap())?;
                peer.set_principal(uri_sans);
            }
        } else {
            let uri_sans_bytes =
                filter.get_property(vec!["connection", "uri_san_peer_certificate"]);
            if uri_sans_bytes.is_none() {
                let dns_sans_bytes =
                    filter.get_property(vec!["connection", "dns_san_peer_certificate"]);
                if dns_sans_bytes.is_none() {
                    let sub_local_bytes = filter
                        .get_property(vec!["connection", "subject_peer_certificate"])
                        .unwrap();
                    let sub_local = String::from_utf8(sub_local_bytes)?;
                    peer.set_principal(sub_local);
                } else {
                    let dns_sans = String::from_utf8(dns_sans_bytes.unwrap())?;
                    peer.set_principal(dns_sans);
                }
            } else {
                let uri_sans = String::from_utf8(uri_sans_bytes.unwrap())?;
                peer.set_principal(uri_sans);
            }
        }
    }

    peer.set_service(service);
    Ok(())
    // Note: X.509 Certificate not included because IDK where to find it.
}

// TODO(rahulanand16nov): This function doesn't rely on hostcalls hence unit testable.
pub fn set_peer_address(peer: &mut AttributeContext_Peer, addr: String, port: u32) {
    let mut socket_address = SocketAddress::new();
    socket_address.set_protocol(SocketAddress_Protocol::TCP);
    socket_address.set_address(addr);
    socket_address.set_ipv4_compat(true); // better to be compatible, right?
    socket_address.set_port_value(port);

    let mut address = Address::new();
    address.set_socket_address(socket_address);
    peer.set_address(address);
}

pub fn set_attribute_request(
    filter: &dyn HttpContext,
    request: &mut AttributeContext_Request,
) -> Result<(), UtilsErr> {
    let request_time_bytes = filter
        .get_property(vec!["request", "time"])
        .unwrap_or_else(|| {
            warn!("request time not found");
            vec![]
        });
    let req_time = read_le_i64(&request_time_bytes)?;
    request.mut_time().set_seconds(req_time / 1_000_000_000);
    request
        .mut_time()
        .set_nanos((req_time % 1_000_000_000).try_into()?); // ER

    let request_id_bytes = filter
        .get_property(vec!["request", "id"])
        .unwrap_or_else(|| {
            warn!("request id not found");
            vec![]
        });
    let request_id = String::from_utf8(request_id_bytes)?;
    request.mut_http().set_id(request_id);

    let request_method_bytes = filter
        .get_property(vec!["request", "method"])
        .unwrap_or_else(|| {
            warn!("request time not found");
            vec![]
        });
    let request_method = String::from_utf8(request_method_bytes)?;
    request.mut_http().set_method(request_method);

    let request_headers: HashMap<_, _> = filter.get_http_request_headers().into_iter().collect();
    request.mut_http().set_headers(request_headers);

    let request_path_bytes = filter
        .get_property(vec!["request", "path"])
        .unwrap_or_else(|| {
            warn!("request path not found");
            vec![]
        });
    let request_path = String::from_utf8(request_path_bytes)?;
    request.mut_http().set_path(request_path);

    let request_host_bytes = filter
        .get_property(vec!["request", "host"])
        .unwrap_or_else(|| {
            warn!("request host not found");
            vec![]
        });
    let request_host = String::from_utf8(request_host_bytes)?;
    request.mut_http().set_host(request_host);

    let request_scheme_bytes = filter
        .get_property(vec!["request", "scheme"])
        .unwrap_or_else(|| {
            warn!("request scheme not found");
            vec![]
        });
    let request_scheme = String::from_utf8(request_scheme_bytes)?;
    request.mut_http().set_scheme(request_scheme);

    let request_protocol_bytes = filter
        .get_property(vec!["request", "protocol"])
        .unwrap_or_else(|| {
            warn!("request protocol not found");
            vec![]
        });
    let request_protocol = String::from_utf8(request_protocol_bytes)?;
    request.mut_http().set_protocol(request_protocol);

    request.mut_http().set_size(-1); // -1 if unknown.
    Ok(())
}

fn match_headers(req_headers: &HashMap<String, String>, config_headers: &[HeaderMatcher]) -> bool {
    for header_matcher in config_headers {
        let invert_match = header_matcher.get_invert_match();
        if let Some(req_header_value) = req_headers.get(header_matcher.get_name()) {
            if let Some(hm_specifier) = &header_matcher.header_match_specifier {
                let mut is_match = false;
                match hm_specifier {
                    HeaderMatcher_specifier::exact_match(str) => is_match = str == req_header_value,
                    HeaderMatcher_specifier::safe_regex_match(regex_matcher) => todo!(), // TODO(rahulanand16nov): not implemented.
                    HeaderMatcher_specifier::range_match(range) => {
                        if let Ok(val) = req_header_value.parse::<i64>() {
                            is_match = range.get_start() <= val && val < range.get_end();
                        }
                    }
                    HeaderMatcher_specifier::present_match(should_be_present) => {
                        is_match = *should_be_present
                    }
                    HeaderMatcher_specifier::prefix_match(prefix) => {
                        is_match = req_header_value.starts_with(prefix.as_str())
                    }
                    HeaderMatcher_specifier::suffix_match(suffix) => {
                        is_match = req_header_value.ends_with(suffix.as_str())
                    }
                    HeaderMatcher_specifier::contains_match(str) => {
                        is_match = req_header_value.contains(str.as_str())
                    }
                    HeaderMatcher_specifier::string_match(str_matcher) => {
                        let ignore_case = str_matcher.get_ignore_case();
                        if let Some(pattern) = &str_matcher.match_pattern {
                            match pattern {
                                StringMatcher_pattern::exact(str) => {
                                    is_match = if ignore_case {
                                        str.eq_ignore_ascii_case(req_header_value)
                                    } else {
                                        str == req_header_value
                                    }
                                }
                                StringMatcher_pattern::prefix(str) => {
                                    is_match = if ignore_case {
                                        req_header_value
                                            .to_lowercase()
                                            .starts_with(&str.to_lowercase())
                                    } else {
                                        req_header_value.starts_with(str.as_str())
                                    }
                                }
                                StringMatcher_pattern::suffix(str) => {
                                    is_match = if ignore_case {
                                        req_header_value
                                            .to_lowercase()
                                            .ends_with(&str.to_lowercase())
                                    } else {
                                        req_header_value.ends_with(str.as_str())
                                    }
                                }
                                StringMatcher_pattern::safe_regex(_) => todo!(), // TODO(rahulanand16nov): not implemented.
                                StringMatcher_pattern::contains(str) => {
                                    is_match = if ignore_case {
                                        req_header_value
                                            .to_lowercase()
                                            .contains(&str.to_lowercase())
                                    } else {
                                        req_header_value.contains(str.as_str())
                                    }
                                }
                            }
                        } else {
                            return false;
                        }
                    }
                }
                if is_match ^ invert_match {
                    return false;
                }
            } else {
                return false;
            }
        } else {
            return false;
        }
    }
    true
}

pub fn descriptor_from_actions(
    filter: &dyn HttpContext,
    actions: &[RLA_action_specifier],
) -> Result<RateLimitDescriptor, UtilsErr> {
    let mut res = RateLimitDescriptor::new();
    for action in actions {
        let mut descriptor_entry = RateLimitDescriptor_Entry::new();

        match action {
            RLA_action_specifier::source_cluster(_) => {
                descriptor_entry.set_key("source_cluster".into());

                let src_cluster = String::from_utf8(
                    filter
                        .get_property(vec!["connection", "requested_server_name"])
                        .unwrap_or_else(|| {
                            warn!("requested service name not found");
                            vec![]
                        }),
                )?; // NOTE: not sure if it's correct.
                descriptor_entry.set_value(src_cluster);
            }
            RLA_action_specifier::destination_cluster(_) => {
                descriptor_entry.set_key("destination_cluster".into());

                let dst_cluster =
                    String::from_utf8(filter.get_property(vec!["cluster_name"]).unwrap_or_else(
                        || {
                            warn!("requested service name not found");
                            vec![]
                        },
                    ))?;
                descriptor_entry.set_value(dst_cluster);
            }
            RLA_action_specifier::request_headers(rh) => {
                descriptor_entry.set_key(rh.get_descriptor_key().into());

                let header_value = filter.get_http_request_header(rh.get_header_name());
                if let Some(value) = header_value {
                    descriptor_entry.set_value(value);
                } else if rh.get_skip_if_absent() {
                    continue; // don't add the descriptor if no match.
                }
            }
            RLA_action_specifier::remote_address(ra) => {
                descriptor_entry.set_key("remote_address".into());

                let header_value = filter.get_http_request_header("x-forwarded-for");
                if let Some(value) = header_value {
                    descriptor_entry.set_value(value);
                } else {
                    continue;
                }
            }
            RLA_action_specifier::generic_key(gk) => {
                descriptor_entry.set_key(gk.get_descriptor_key().into());
                descriptor_entry.set_value(gk.get_descriptor_value().into());
            }
            RLA_action_specifier::header_value_match(hvm) => {
                let request_headers: HashMap<_, _> =
                    filter.get_http_request_headers().into_iter().collect();

                if hvm.get_expect_match().get_value()
                    == match_headers(&request_headers, hvm.get_headers())
                {
                    descriptor_entry.set_key("header_match".into());
                    descriptor_entry.set_value(hvm.get_descriptor_value().into());
                } else {
                    continue;
                }
            }
            RLA_action_specifier::dynamic_metadata(_) => todo!(),
            RLA_action_specifier::metadata(_) => todo!(),
            RLA_action_specifier::extension(_) => todo!(),
        }
        res.mut_entries().push(descriptor_entry);
    }
    Ok(res)
}
