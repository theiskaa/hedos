//! `hedos bench [model...]` — what every model on this machine actually does:
//! tokens a second, time to first token, and cold start, measured the same way
//! for every row so two of them can be compared.

use std::io::IsTerminal;
use std::sync::Arc;

use clap::Args;
use kernel::bench::{ColdStart, Row, Status, TimingSource};
use kernel::records::{Capability, ModelRecord};
use ratatui::backend::CrosstermBackend;
use ratatui::widgets::Paragraph;
use ratatui::{Terminal, TerminalOptions, Viewport};
use runtime::bench::{BenchPlan, Cancel, DEFAULT_MAX_TOKENS, DEFAULT_PROMPT, DEFAULT_RUNS};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::error::CliError;
use crate::support::bench_run::{ShelfPrepare, machine_line, row_for, rows as shelf_rows};
use crate::support::bench_view::{Board, plain, windowed};
use crate::support::machine;
use crate::support::output::Out;
use crate::support::session::{self, Session};
use crate::support::signals;

/// How often the live block redraws for its own sake, so the spinner turns
/// while a slow model is still on its first token.
const TICK: std::time::Duration = std::time::Duration::from_millis(120);

/// Arguments for `bench`.
#[derive(Args)]
pub struct BenchArgs {
    /// The models to bench (name, alias, or id). Omit for every chat model
    /// that fits this machine.
    models: Vec<String>,
    /// Bench the models that do not fit this machine's memory too.
    #[arg(long)]
    all: bool,
    /// Warm runs per model, after the cold one.
    #[arg(long, default_value_t = DEFAULT_RUNS)]
    runs: usize,
    /// Cap each reply at this many tokens.
    #[arg(long, default_value_t = DEFAULT_MAX_TOKENS)]
    max_tokens: i64,
    /// The prompt every model answers.
    #[arg(long)]
    prompt: Option<String>,
    /// Leave residency alone: nothing is evicted, so nothing is measured cold.
    #[arg(long)]
    keep_warm: bool,
}

/// Run the `bench` command.
pub async fn run(args: BenchArgs, out: &Out) -> Result<(), CliError> {
    let session = Arc::new(Session::open()?);
    let shelf = session.shelf_or_discover().await?;
    let rows = select(&args, &shelf)?;
    if rows
        .iter()
        .all(|row| matches!(row.status, Status::Skipped(_)))
    {
        return Err(CliError::new(
            "no model here can be benched — `hedos ls` shows what is on the shelf".to_owned(),
        ));
    }

    let plan = BenchPlan {
        models: rows
            .iter()
            .filter(|row| row.status == Status::Waiting)
            .map(|row| row.id.clone())
            .collect(),
        prompt: args
            .prompt
            .clone()
            .unwrap_or_else(|| DEFAULT_PROMPT.to_owned()),
        max_tokens: args.max_tokens,
        runs: args.runs.max(1),
        keep_warm: args.keep_warm,
    };
    let board = Board::new(
        rows,
        plan.runs,
        plan.max_tokens,
        machine_line(machine::memory_budget_bytes() as i64),
    );
    let board = drive(board, plan.clone(), &session, &shelf, out).await?;

    if out.is_json() {
        out.json(&document(&board, &plan));
    } else if !std::io::stdout().is_terminal() {
        out.line(&plain(&board.rows));
    }
    if board.rows.iter().any(|row| row.status.rate().is_some()) {
        Ok(())
    } else {
        Err(CliError::new("no model produced a figure".to_owned()))
    }
}

/// Run the bench, drawing it live on a terminal and quietly otherwise, and
/// hand back the board it ended on.
async fn drive(
    mut board: Board,
    plan: BenchPlan,
    session: &Arc<Session>,
    shelf: &[ModelRecord],
    out: &Out,
) -> Result<Board, CliError> {
    let prepare = ShelfPrepare::over(session, shelf, &plan.models);
    let cancel = Cancel::new();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let driver = {
        let (session, cancel, plan) = (Arc::clone(session), cancel.clone(), plan);
        tokio::spawn(async move {
            runtime::bench::run(&session.kernel, &plan, &prepare, &cancel, &tx).await;
        })
    };

    let live = !out.is_json() && std::io::stdout().is_terminal();
    let mut terminal = live.then(|| open_block(&board)).transpose()?;
    // What ratatui actually gave the block, which on a short terminal is less
    // than the table asked for.
    let block_rows = terminal
        .as_mut()
        .map_or(0, |terminal| terminal.get_frame().area().height);
    let mut ticker = tokio::time::interval(TICK);
    let mut ticks = 0u64;
    let mut stopping = false;
    // The table is on screen before the first model is: opening a shelf and
    // clearing a model from memory take seconds, and a blank terminal for that
    // long reads as a command that did nothing.
    if let Some(terminal) = terminal.as_mut() {
        redraw(terminal, &board, ticks, false)?;
    }
    loop {
        let mut moved = false;
        tokio::select! {
            received = rx.recv() => match received {
                Some(event) => moved = board.apply(&event),
                None => break,
            },
            _ = ticker.tick() => {
                ticks += 1;
                // Redrawn until the last row settles, not only while one is
                // generating: the spinner has to turn through the eviction and
                // the load as well, which is the longest part of a cold start.
                moved = !board.finished();
            }
            // Ctrl-C stops the bench, and what has been measured stands. A
            // second one gives up on the run in flight rather than waiting on a
            // backend that may never answer.
            () = signals::wait_for_ctrl_c() => {
                if stopping {
                    break;
                }
                cancel.stop();
                stopping = true;
            }
        }
        if let (true, Some(terminal)) = (moved, terminal.as_mut()) {
            redraw(terminal, &board, ticks, false)?;
        }
    }
    let _ = driver.await;

    if let Some(terminal) = terminal.as_mut() {
        redraw(terminal, &board, ticks, true)?;
    }
    if terminal.take().is_some() {
        // The block stays where it was drawn; the cursor is walked past it so
        // the shell's next prompt does not land on the table's last row.
        print!("{}", "\n".repeat(usize::from(block_rows)));
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }
    Ok(board)
}

/// An inline block the height of the table, capped at what the terminal has:
/// asking for more scrolls that much of the user's scrollback away before a
/// single row is drawn.
fn open_block(board: &Board) -> Result<Terminal<CrosstermBackend<std::io::Stdout>>, CliError> {
    let backend = CrosstermBackend::new(std::io::stdout());
    let terminal_rows = ratatui::backend::Backend::size(&backend)
        .map(|size| size.height)
        .unwrap_or(u16::MAX);
    let rows = board.height().min(terminal_rows);
    // Raw mode is deliberately not enabled: nothing here reads a key, and
    // leaving the line discipline alone keeps Ctrl-C a signal this command can
    // handle rather than a keystroke it would have to poll for.
    Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(rows),
        },
    )
    .map_err(|error| CliError::new(format!("terminal error: {error}")))
}

fn redraw(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    board: &Board,
    ticks: u64,
    settled: bool,
) -> Result<(), CliError> {
    terminal
        .draw(|frame| {
            let area = frame.area();
            let lines = board.lines(area.width as usize, ticks, settled);
            let lines = windowed(lines, area.height as usize, board.running());
            frame.render_widget(Paragraph::new(lines), area);
        })
        .map(|_| ())
        .map_err(|error| CliError::new(format!("terminal error: {error}")))
}

/// The rows to bench: the models named, or every chat model on the shelf.
///
/// A model named on the command line is benched whatever its fit says: the
/// reason to name one is often to find out what it does here.
fn select(args: &BenchArgs, shelf: &[ModelRecord]) -> Result<Vec<Row>, CliError> {
    if args.models.is_empty() {
        return Ok(shelf_rows(shelf, machine::memory_budget_bytes(), args.all));
    }
    let budget = machine::memory_budget_bytes();
    let mut rows: Vec<Row> = Vec::with_capacity(args.models.len());
    for query in &args.models {
        let record = session::resolve(query, shelf, Some(&Capability::chat()))?;
        // Two names for one model are one row: a second would be benched, and
        // then never matched to, since a step is applied to the first row of
        // its id.
        if rows.iter().any(|row| row.id == record.id) {
            continue;
        }
        rows.push(row_for(record, budget, true));
    }
    Ok(rows)
}

/// The whole bench as one JSON document.
fn document(board: &Board, plan: &BenchPlan) -> Value {
    json!({
        "machine": {
            "chip": runtime::chip::name(),
            "memoryBytes": machine::memory_budget_bytes(),
        },
        "prompt": plan.prompt,
        "maxTokens": plan.max_tokens,
        "runs": plan.runs,
        "keepWarm": plan.keep_warm,
        "models": board.rows.iter().map(model_json).collect::<Vec<_>>(),
    })
}

fn model_json(row: &Row) -> Value {
    let mut model = json!({
        "id": row.id,
        "name": row.name,
        "runtime": row.runtime,
        "quantization": row.quantization,
        "status": status_name(&row.status),
    });
    let object = model.as_object_mut().expect("a model object");
    match &row.status {
        Status::Done(figures) => {
            object.insert("coldStart".to_owned(), cold_json(&figures.cold_start));
            object.insert(
                "tokensPerSecond".to_owned(),
                measure_json(
                    figures.tokens_per_second,
                    Some(figures.estimated_tokens),
                    Some(figures.source),
                ),
            );
            object.insert(
                "ttftMs".to_owned(),
                measure_json(figures.ttft_ms, None, None),
            );
            object.insert(
                "promptTokensPerSecond".to_owned(),
                measure_json(figures.prompt_tokens_per_second, None, None),
            );
            object.insert(
                "runs".to_owned(),
                Value::Array(figures.runs.iter().map(run_json).collect()),
            );
        }
        Status::Failed(reason) | Status::Skipped(reason) => {
            object.insert("reason".to_owned(), Value::String(reason.clone()));
        }
        Status::Waiting | Status::Running { .. } | Status::Stopped => {}
    }
    model
}

fn status_name(status: &Status) -> &'static str {
    match status {
        Status::Waiting => "waiting",
        Status::Running { .. } => "running",
        Status::Done(_) => "done",
        Status::Failed(_) => "failed",
        Status::Skipped(_) => "skipped",
        Status::Stopped => "stopped",
    }
}

fn cold_json(cold: &ColdStart) -> Value {
    match cold {
        ColdStart::Measured(ms) => json!({ "millis": ms }),
        ColdStart::Held(holder) => json!({ "heldBy": holder }),
        ColdStart::NotMeasured => Value::Null,
    }
}

fn measure_json(
    measure: Option<kernel::bench::Measure>,
    estimated: Option<bool>,
    source: Option<TimingSource>,
) -> Value {
    let Some(measure) = measure else {
        return Value::Null;
    };
    let mut value = json!({
        "median": measure.median,
        "min": measure.min,
        "max": measure.max,
    });
    let object = value.as_object_mut().expect("a measure object");
    if let Some(estimated) = estimated {
        object.insert("estimated".to_owned(), Value::Bool(estimated));
    }
    if let Some(source) = source {
        object.insert(
            "source".to_owned(),
            Value::String(source.as_str().to_owned()),
        );
    }
    value
}

fn run_json(sample: &kernel::bench::Sample) -> Value {
    json!({
        "completionTokens": sample.completion_tokens,
        "promptTokens": sample.prompt_tokens,
        "ttftMs": sample.ttft_ms,
        "decodeMs": sample.decode_ms,
        "promptMs": sample.prompt_ms,
        "estimatedTokens": sample.estimated_tokens,
        "source": sample.source.as_str(),
    })
}
