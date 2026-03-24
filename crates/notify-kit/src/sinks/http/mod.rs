mod body;
mod client;
mod public_ip;
mod url;

// Keep sink imports stable while splitting HTTP helpers by responsibility.
pub(crate) use body::{
    DEFAULT_MAX_RESPONSE_BODY_BYTES, ensure_http_success, http_status_text_error,
    read_json_body_after_http_success, read_json_body_limited, read_text_body_limited,
    response_body_read_error,
};
pub(crate) use client::{build_http_client, select_http_client, send_reqwest};
pub(crate) use url::{
    parse_and_validate_https_url, parse_and_validate_https_url_basic, redact_url, redact_url_str,
    validate_url_path_prefix,
};
