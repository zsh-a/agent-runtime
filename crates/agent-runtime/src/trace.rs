use agent_core::{AgentError, TraceEvent, TraceSink};
use async_trait::async_trait;
use tokio::sync::{Mutex, broadcast};

#[derive(Default)]
pub struct TraceEventBuffer {
    events: Mutex<Vec<TraceEvent>>,
}

impl TraceEventBuffer {
    pub(crate) async fn push(&self, event: TraceEvent) {
        self.events.lock().await.push(event);
    }

    pub async fn events(&self) -> Vec<TraceEvent> {
        self.events.lock().await.clone()
    }
}

#[derive(Default)]
pub struct MemoryTraceSink {
    events: Mutex<Vec<TraceEvent>>,
    event_sender: Option<broadcast::Sender<TraceEvent>>,
    event_buffer: Option<std::sync::Arc<TraceEventBuffer>>,
}

impl MemoryTraceSink {
    pub fn with_event_sender(
        event_sender: broadcast::Sender<TraceEvent>,
        event_buffer: Option<std::sync::Arc<TraceEventBuffer>>,
    ) -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            event_sender: Some(event_sender),
            event_buffer,
        }
    }

    pub fn with_event_buffer(event_buffer: std::sync::Arc<TraceEventBuffer>) -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            event_sender: None,
            event_buffer: Some(event_buffer),
        }
    }

    pub async fn events(&self) -> Vec<TraceEvent> {
        self.events.lock().await.clone()
    }
}

#[async_trait]
impl TraceSink for MemoryTraceSink {
    async fn emit(&self, event: TraceEvent) -> Result<(), AgentError> {
        {
            self.events.lock().await.push(event.clone());
        }
        if let Some(buffer) = &self.event_buffer {
            buffer.push(event.clone()).await;
        }
        if let Some(sender) = &self.event_sender {
            let _ = sender.send(event);
        }
        Ok(())
    }
}
