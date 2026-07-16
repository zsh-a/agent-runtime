mod context;
mod error;
mod events;
mod runner;
mod state;
mod types;

pub use error::{ChatError, ChatErrorRecord};
pub use runner::ChatTurnRunner;
pub use state::{
    chat_turn_apply_response, chat_turn_apply_tool_results, chat_turn_initial_state,
    chat_turn_llm_request, chat_turn_next_round, chat_turn_prepare_llm_request,
};
pub use types::{
    ChatEventStream, ChatResumeRequest, ChatToolCall, ChatToolExecution, ChatToolResult,
    ChatTurnAdvance, ChatTurnEvent, ChatTurnEventKind, ChatTurnRequest, ChatTurnState,
};

pub(crate) use events::{
    chat_event_from_llm_event, send_done, send_error, send_event, turn_metadata,
};
pub(crate) use state::ToolOutput;

#[cfg(test)]
mod tests;
