#![forbid(unsafe_code)]

mod body;
mod client;
mod error;
mod http_probe;
mod ip;
mod outbound_policy;
mod public_ip;
mod sse;
mod tokio_time;
mod url;

pub use body::{
    DEFAULT_MAX_RESPONSE_BODY_BYTES, ReadReqwestBodyBytesError, body_preview_json,
    body_preview_text, drain_response_body, ensure_http_success, http_status_text_error,
    read_json_body_after_http_success, read_json_body_after_http_success_limited,
    read_json_body_limited, read_reqwest_body_bytes_limited, read_reqwest_body_bytes_truncated,
    read_response_body_preview_text, read_text_body_limited, response_body_read_error,
    send_reqwest_after_http_success, send_reqwest_json_after_http_success,
    send_reqwest_json_after_http_success_limited, send_reqwest_text_after_http_success,
    send_reqwest_text_after_http_success_limited, write_response_body_limited,
};
pub use client::{
    HttpClientOptions, HttpClientProfile, build_http_client, build_http_client_profile,
    build_http_client_with_options, select_http_client_from_profile,
    select_http_client_with_options, send_reqwest,
};
pub use error::{Error, ErrorKind, Result};
pub use http_probe::{
    HttpProbeKind, HttpProbeMethod, HttpProbeResult, probe_http_endpoint,
    probe_http_endpoint_detailed,
};
pub use outbound_policy::{
    UntrustedOutboundError, UntrustedOutboundPolicy, validate_untrusted_outbound_url,
    validate_untrusted_outbound_url_dns,
};
pub use sse::{
    SseLimits, sse_data_stream_from_reader, sse_data_stream_from_reader_with_limits,
    sse_data_stream_from_response,
};
pub use url::{
    WebsocketBaseUrlResolution, WebsocketBaseUrlRewrite, append_url_query_params,
    append_url_query_params_encoded, join_api_base_url_path, parse_and_validate_https_url,
    parse_and_validate_https_url_basic, redact_reqwest_error, redact_url, redact_url_for_error,
    redact_url_str, resolve_websocket_base_url, validate_url_path_prefix,
};
