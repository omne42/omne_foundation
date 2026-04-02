use serde_json::Value;

pub(crate) const PING_METHOD: &str = "ping";
pub(crate) const TOOLS_LIST_METHOD: &str = "tools/list";
pub(crate) const TOOLS_CALL_METHOD: &str = "tools/call";
pub(crate) const RESOURCES_LIST_METHOD: &str = "resources/list";
pub(crate) const RESOURCES_TEMPLATES_LIST_METHOD: &str = "resources/templates/list";
pub(crate) const RESOURCES_READ_METHOD: &str = "resources/read";
pub(crate) const RESOURCES_SUBSCRIBE_METHOD: &str = "resources/subscribe";
pub(crate) const RESOURCES_UNSUBSCRIBE_METHOD: &str = "resources/unsubscribe";
pub(crate) const PROMPTS_LIST_METHOD: &str = "prompts/list";
pub(crate) const PROMPTS_GET_METHOD: &str = "prompts/get";
pub(crate) const LOGGING_SET_LEVEL_METHOD: &str = "logging/setLevel";
pub(crate) const COMPLETION_COMPLETE_METHOD: &str = "completion/complete";

pub(crate) fn uri_params(uri: &str) -> Value {
    serde_json::json!({ "uri": uri })
}

pub(crate) fn prompt_get_params(prompt: &str, arguments: Option<Value>) -> Value {
    let mut params = serde_json::json!({ "name": prompt });
    if let Some(arguments) = arguments {
        params["arguments"] = arguments;
    }
    params
}

pub(crate) fn tool_call_params(tool: &str, arguments: Option<Value>) -> Value {
    let mut params = serde_json::json!({ "name": tool });
    if let Some(arguments) = arguments {
        params["arguments"] = arguments;
    }
    params
}

pub(crate) fn logging_level_params(level: &str) -> Value {
    serde_json::json!({ "level": level })
}
