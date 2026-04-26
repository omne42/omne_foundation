#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputTrigger {
    pub trigger_id: String,
    pub label: Option<String>,
    pub kind: InputTriggerKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum InputTriggerKind {
    WindowButton,
    GlobalShortcut { accelerator: String },
    VoiceActivation { wake_phrase: String },
    TrayMenu { item_id: String },
    ExternalCommand { command_id: String },
    Custom { id: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceActivationSettings {
    pub enabled: bool,
    pub wake_phrase: String,
    pub engine: VoiceActivationEngine,
    pub speech_threshold: f32,
    pub min_wake_speech_ms: u64,
    pub wake_silence_ms: u64,
    pub wake_probe_cooldown_ms: u64,
    pub dictation_silence_ms: u64,
    pub no_speech_timeout_ms: u64,
    pub max_dictation_ms: u64,
    pub max_wake_probe_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum VoiceActivationEngine {
    TranscriptionProbe,
    WebSpeechRecognition,
    NativeWakeWord { model_id: Option<String> },
    Custom { id: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceActivationEvent {
    pub event_id: String,
    pub wake_phrase: String,
    pub transcript: Option<String>,
    pub confidence: Option<f32>,
    pub occurred_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputTriggerEvent {
    pub event_id: String,
    pub trigger: InputTrigger,
    pub occurred_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TextDeliveryTarget {
    Clipboard,
    AppDraft,
    DirectInsert,
    Custom { id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDeliveryRequest {
    pub text: String,
    pub target: TextDeliveryTarget,
    pub source_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDeliveryResult {
    pub target: TextDeliveryTarget,
    pub delivered: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPermission {
    pub kind: DesktopPermissionKind,
    pub required: bool,
    pub granted: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DesktopPermissionKind {
    GlobalShortcut,
    Tray,
    Microphone,
    SpeechRecognition,
    Clipboard,
    Accessibility,
    InputSimulation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopInputError {
    pub kind: DesktopInputErrorKind,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DesktopInputErrorKind {
    ShortcutConflict,
    MicrophoneUnavailable,
    WakePhraseUnavailable,
    ClipboardUnavailable,
    AccessibilityPermissionMissing,
    PermissionDenied,
    UnsupportedPlatform,
    TargetUnavailable,
    AdapterUnavailable,
    Internal,
}

#[cfg(test)]
mod tests {
    use super::{
        DesktopInputError, DesktopInputErrorKind, DesktopPermission, DesktopPermissionKind,
        InputTrigger, InputTriggerEvent, InputTriggerKind, TextDeliveryRequest, TextDeliveryTarget,
        VoiceActivationEngine, VoiceActivationEvent, VoiceActivationSettings,
    };

    #[test]
    fn trigger_event_serializes_stable_shortcut_shape() {
        let event = InputTriggerEvent {
            event_id: "event-1".to_string(),
            trigger: InputTrigger {
                trigger_id: "record-toggle".to_string(),
                label: Some("Record".to_string()),
                kind: InputTriggerKind::GlobalShortcut {
                    accelerator: "Alt+Space".to_string(),
                },
            },
            occurred_at: Some("2026-04-26T00:00:00Z".to_string()),
        };

        let value = serde_json::to_value(event).expect("serialize trigger event");

        assert_eq!(value["eventId"], "event-1");
        assert_eq!(value["trigger"]["kind"]["kind"], "globalShortcut");
        assert_eq!(value["trigger"]["kind"]["accelerator"], "Alt+Space");
    }

    #[test]
    fn text_delivery_request_serializes_target() {
        let request = TextDeliveryRequest {
            text: "hello".to_string(),
            target: TextDeliveryTarget::Clipboard,
            source_id: Some("job-1".to_string()),
        };

        let value = serde_json::to_value(request).expect("serialize delivery request");

        assert_eq!(value["text"], "hello");
        assert_eq!(value["target"]["kind"], "clipboard");
        assert_eq!(value["sourceId"], "job-1");
    }

    #[test]
    fn voice_activation_serializes_stable_contract_shape() {
        let settings = VoiceActivationSettings {
            enabled: true,
            wake_phrase: "typemic".to_string(),
            engine: VoiceActivationEngine::TranscriptionProbe,
            speech_threshold: 0.018,
            min_wake_speech_ms: 650,
            wake_silence_ms: 900,
            wake_probe_cooldown_ms: 900,
            dictation_silence_ms: 1200,
            no_speech_timeout_ms: 12000,
            max_dictation_ms: 120000,
            max_wake_probe_ms: 3500,
        };
        let event = VoiceActivationEvent {
            event_id: "wake-1".to_string(),
            wake_phrase: "typemic".to_string(),
            transcript: Some("typemic".to_string()),
            confidence: None,
            occurred_at: Some("2026-04-26T00:00:00Z".to_string()),
        };

        let settings_value =
            serde_json::to_value(settings).expect("serialize voice activation settings");
        let event_value = serde_json::to_value(event).expect("serialize voice activation event");

        assert_eq!(settings_value["enabled"], true);
        assert_eq!(settings_value["wakePhrase"], "typemic");
        assert_eq!(settings_value["engine"]["kind"], "transcriptionProbe");
        assert_eq!(settings_value["minWakeSpeechMs"], 650);
        assert_eq!(settings_value["wakeProbeCooldownMs"], 900);
        assert_eq!(settings_value["noSpeechTimeoutMs"], 12000);
        assert_eq!(event_value["wakePhrase"], "typemic");
        assert_eq!(event_value["transcript"], "typemic");
    }

    #[test]
    fn permission_and_error_use_stable_wire_names() {
        let permission = DesktopPermission {
            kind: DesktopPermissionKind::Accessibility,
            required: true,
            granted: Some(false),
        };
        let error = DesktopInputError {
            kind: DesktopInputErrorKind::AccessibilityPermissionMissing,
            message: "accessibility permission is missing".to_string(),
            retryable: false,
        };

        let permission_value = serde_json::to_value(permission).expect("serialize permission");
        let error_value = serde_json::to_value(error).expect("serialize error");

        assert_eq!(permission_value["kind"], "accessibility");
        assert_eq!(permission_value["required"], true);
        assert_eq!(error_value["kind"], "accessibilityPermissionMissing");
        assert_eq!(error_value["retryable"], false);
    }
}
