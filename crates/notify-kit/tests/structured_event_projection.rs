use std::pin::Pin;
use std::sync::{Arc, Mutex};

use notify_kit::{Event, Hub, HubConfig, Result, Severity, Sink};
use structured_text_kit::structured_text;

#[derive(Clone, Debug, PartialEq, Eq)]
struct CapturedEvent {
    title: String,
    body: Option<String>,
    tags: Vec<(String, String)>,
}

struct CapturingSink {
    captured: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl Sink for CapturingSink {
    fn name(&self) -> &'static str {
        "capture"
    }

    fn send<'a>(
        &'a self,
        event: &'a Event,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        let captured = self.captured.clone();
        Box::pin(async move {
            let snapshot = CapturedEvent {
                title: event.title().to_string(),
                body: event.body().map(str::to_owned),
                tags: event
                    .tags()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            };
            captured.lock().expect("lock capture buffer").push(snapshot);
            Ok(())
        })
    }
}

#[tokio::test(flavor = "current_thread")]
async fn core_sinks_consume_structured_event_via_string_projection_accessors() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let hub = Hub::new(
        HubConfig::default(),
        vec![Arc::new(CapturingSink {
            captured: captured.clone(),
        }) as Arc<dyn Sink>],
    );

    let title = structured_text!("notify.title", "repo" => "omne");
    let body = structured_text!("notify.body", "step" => "review");
    let tag = structured_text!("notify.tag", "value" => "t1");
    let event = Event::new_structured("kind", Severity::Info, title.clone())
        .with_body_text(body.clone())
        .with_tag_text("thread_id", tag.clone());

    hub.send(event).await.expect("send structured event");

    let snapshots = captured.lock().expect("lock capture buffer");
    assert_eq!(
        snapshots.as_slice(),
        &[CapturedEvent {
            title: title.to_string(),
            body: Some(body.to_string()),
            tags: vec![("thread_id".to_string(), tag.to_string())],
        }]
    );
}
