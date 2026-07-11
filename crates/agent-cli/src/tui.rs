mod approval;
mod chat;
mod chat_events;
mod commands;
mod completion;
mod data;
mod format;
mod layout;
mod policy;
mod render;
mod runtime;
mod terminal;
#[cfg(test)]
mod test_support;
mod theme;
mod tool_inventory;

use miette::Result;

pub(crate) use data::TuiOptions;
use data::TuiState;
use render::render_tui_once;
use terminal::run_tui_terminal;

pub(crate) async fn run_tui(options: TuiOptions) -> Result<()> {
    let once = options.once;
    let state = TuiState::load(options).await?;
    if once {
        println!("{}", render_tui_once(&state)?);
        return Ok(());
    }
    run_tui_terminal(state).await
}
