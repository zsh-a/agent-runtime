use miette::{IntoDiagnostic, Result};
use ratatui::{
    Frame, Terminal,
    backend::TestBackend,
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Wrap,
    },
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{
    data::{
        TranscriptItem, TranscriptRole, TuiActivityItem, TuiActivityKind, TuiApprovalSelection,
        TuiDetailKind, TuiFocusPanel, TuiPendingApprovalAction, TuiProposalListSummary,
        TuiRunSummary, TuiState, TuiTextSelection, TuiTraceEventSummary, TuiWorkflowSummary,
    },
    layout::TuiLayout,
    theme,
};

const MAX_INPUT_HEIGHT: u16 = 8;
const MIN_INPUT_HEIGHT: u16 = 3;
const INPUT_PREFIX_WIDTH: u16 = 2;

mod approval_overlay;
mod frame;
mod input;
mod selection;
mod styles;
mod transcript;

pub(super) use approval_overlay::*;
pub(super) use frame::*;
pub(super) use input::*;
pub(super) use selection::*;
pub(super) use styles::*;
pub(super) use transcript::*;

#[cfg(test)]
#[path = "tests/render.rs"]
mod tests;
