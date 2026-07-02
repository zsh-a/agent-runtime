use agent_core::{AgentError, TraceEvent, TraceSink};
use async_trait::async_trait;
use tokio::sync::{Mutex, broadcast};

#[derive(Default)]
pub struct MemoryTraceSink {
    events: Mutex<Vec<TraceEvent>>,
    event_sender: Option<broadcast::Sender<TraceEvent>>,
}

impl MemoryTraceSink {
    pub fn with_event_sender(event_sender: broadcast::Sender<TraceEvent>) -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            event_sender: Some(event_sender),
        }
    }

    pub async fn events(&self) -> Vec<TraceEvent> {
        self.events.lock().await.clone()
    }
}

#[async_trait]
impl TraceSink for MemoryTraceSink {
    async fn emit(&self, event: TraceEvent) -> Result<(), AgentError> {
        self.events.lock().await.push(event.clone());
        if let Some(sender) = &self.event_sender {
            let _ = sender.send(event);
        }
        Ok(())
    }
}
