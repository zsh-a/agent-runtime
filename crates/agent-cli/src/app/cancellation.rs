use std::sync::Arc;

use agent_core::{AgentCancellation, CancellationFuture, CancellationSignal};
use tokio_util::sync::CancellationToken;

pub(crate) fn agent_cancellation(token: CancellationToken) -> AgentCancellation {
    AgentCancellation::new(Arc::new(TokioCancellation { token }))
}

struct TokioCancellation {
    token: CancellationToken,
}

impl CancellationSignal for TokioCancellation {
    fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    fn cancelled(&self) -> CancellationFuture<'_> {
        Box::pin(self.token.cancelled())
    }
}
