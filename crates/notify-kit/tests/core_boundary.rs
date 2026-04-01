use std::sync::Arc;
use std::{future::Future, pin::Pin};

use notify_kit::core::{Event, Hub, HubConfig, Severity, Sink};

struct TestSink;

impl Sink for TestSink {
    fn name(&self) -> &'static str {
        "test"
    }

    fn send<'a>(
        &'a self,
        _event: &'a Event,
    ) -> Pin<Box<dyn Future<Output = notify_kit::core::Result<()>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test(flavor = "current_thread")]
async fn core_namespace_supports_hub_without_builtin_sinks() {
    let hub = Hub::new(
        HubConfig::default(),
        vec![Arc::new(TestSink) as Arc<dyn Sink>],
    );

    hub.send(Event::new("kind", Severity::Info, "title"))
        .await
        .expect("core hub send should succeed");
}

#[test]
fn builtin_namespace_exposes_env_bootstrap() {
    let _ = notify_kit::builtin::env::StandardEnvHubOptions::default();
}
