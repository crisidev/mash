use std::sync::{Arc, RwLock};

use rustyline::completion::Completer;
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{CompletionType, Config, Editor, Helper};
use tokio::sync::mpsc;

use crate::completion::{self, CompletionState};

#[derive(Clone)]
pub(crate) enum InputRequest {
    ReadLine { prompt: String },
    Shutdown,
}

#[derive(Debug)]
pub(crate) enum InputEvent {
    Line(String),
    Eof,
    Interrupted,
}

struct MashHelper {
    state: Arc<RwLock<CompletionState>>,
}

impl Helper for MashHelper {}
impl Highlighter for MashHelper {}
impl Hinter for MashHelper {
    type Hint = String;
}
impl Validator for MashHelper {}

impl Completer for MashHelper {
    type Candidate = String;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<String>)> {
        let state = match self.state.read() {
            Ok(s) => s,
            Err(_) => return Ok((0, Vec::new())),
        };

        // Find the start of the current word
        let start = line[..pos].rfind([' ', '\t']).map(|i| i + 1).unwrap_or(0);
        let text = &line[start..pos];

        let candidates = completion::complete_line(line, text, &state);
        Ok((start, candidates))
    }
}

pub(crate) fn spawn_input_thread(
    completion_state: Arc<RwLock<CompletionState>>,
) -> (mpsc::Sender<InputRequest>, mpsc::Receiver<InputEvent>) {
    let (req_tx, mut req_rx) = mpsc::channel::<InputRequest>(1);
    let (resp_tx, resp_rx) = mpsc::channel::<InputEvent>(1);

    std::thread::spawn(move || {
        let config = Config::builder().completion_type(CompletionType::List).build();
        let mut rl = Editor::with_config(config).unwrap();
        let helper = MashHelper {
            state: completion_state,
        };
        rl.set_helper(Some(helper));

        let histfile = dirs_histfile();
        rl.load_history(&histfile).ok();

        while let Some(req) = req_rx.blocking_recv() {
            match req {
                InputRequest::ReadLine { prompt } => match rl.readline(&prompt) {
                    Ok(line) => {
                        rl.add_history_entry(&line).ok();
                        resp_tx.blocking_send(InputEvent::Line(line)).ok();
                    }
                    Err(ReadlineError::Eof) => {
                        resp_tx.blocking_send(InputEvent::Eof).ok();
                    }
                    Err(ReadlineError::Interrupted) => {
                        resp_tx.blocking_send(InputEvent::Interrupted).ok();
                    }
                    Err(_) => {
                        resp_tx.blocking_send(InputEvent::Eof).ok();
                    }
                },
                InputRequest::Shutdown => {
                    rl.save_history(&histfile).ok();
                    break;
                }
            }
        }
    });

    (req_tx, resp_rx)
}

fn dirs_histfile() -> String {
    etcetera::home_dir()
        .map(|d| d.join(".mash_history").to_string_lossy().to_string())
        .unwrap_or_else(|_| ".mash_history".to_string())
}
