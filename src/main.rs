//! field-eval — offline tactical LLM and survival telemetry utility.
//!
//! One binary, no daemon, no sockets. Model weights are memory-mapped from
//! local storage; the reference corpus is compiled into the executable.

mod corpus;
mod tui;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rig_agent::agent::MultiTurnStreamItem;
use rig_agent::AgentBuilder;
use rig_core::message::Message;
use rig_core::streaming::{Item, StreamEvent};
use rig_llama_cpp::{Client, Model};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

use tui::{Action, App, Role, StreamMsg, Tui};

const MAX_TOKENS: u64 = 768;
/// Shorter budget for one-shot `--ask`; a phone-class CPU decodes slowly.
const ASK_MAX_TOKENS: u64 = 320;
const TOP_K: usize = 3;
/// Default context window. Overridden by `--ctx` or `FIELD_EVAL_CTX`.
const DEFAULT_CTX: u32 = 4096;

const SYSTEM_RULES: &str = "\
You are FIELD-EVAL, an offline field reference assistant running on a handheld device.

Rules:
- Answer only from the retrieved manual sections supplied below. Do not use outside knowledge.
- Cite each claim as [<file> \u{00a7} <heading>], copying the exact filename and heading from the sections above (for example [vehicle.md \u{00a7} Brakes]). Never invent a section number.
- Be terse: numbered steps, imperative voice, no preamble, no closing offer.
- If the sections do not cover the question, say what is missing and name the manual that should be consulted.
- Assume no network, no backup, and limited supplies. Prefer the action that keeps the user alive longest.";

struct Args {
    model: Option<String>,
    ask: Option<String>,
    selftest: bool,
    ctx: u32,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        model: std::env::var("FIELD_EVAL_MODEL").ok(),
        ask: None,
        selftest: false,
        ctx: std::env::var("FIELD_EVAL_CTX")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_CTX),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--model" | "-m" => {
                args.model = Some(it.next().ok_or("--model needs a path")?);
            }
            "--ask" | "-a" => {
                args.ask = Some(it.next().ok_or("--ask needs a question")?);
            }
            "--selftest" => args.selftest = true,
            "--ctx" => {
                args.ctx = it
                    .next()
                    .ok_or("--ctx needs a token count")?
                    .parse()
                    .map_err(|_| "--ctx must be a number")?;
            }
            "--help" | "-h" => {
                println!(
                    "field-eval {}\n\nUSAGE:\n  field-eval [--model PATH] [--ctx N] [--ask QUERY]\n  field-eval --selftest\n\nENV:\n  FIELD_EVAL_MODEL   path to a GGUF model\n  FIELD_EVAL_CTX     context window in tokens (default 4096)\n\nKEYS:\n  enter ask · tab theme · pgup/pgdn scroll · ^l clear · ^c quit",
                    env!("CARGO_PKG_VERSION")
                );
                std::process::exit(0);
            }
            "--version" | "-V" => {
                println!("field-eval {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(args)
}

/// A loaded GGUF together with the client that owns its inference worker.
///
/// `Client::drop` shuts the worker down synchronously, so the client must
/// outlive every `Model` cloned from it. Extracting the model from a
/// temporary client compiles fine and then fails every request with
/// "Inference thread shut down".
pub struct Loaded {
    _client: Client,
    model: Model,
}

fn load_model(path: &str, n_ctx: u32) -> Result<Loaded, String> {
    if !std::path::Path::new(path).exists() {
        return Err(format!("model not found: {path}"));
    }
    let client = Client::builder(path)
        .n_ctx(n_ctx.clamp(512, 32768))
        .build()
        .map_err(|e| format!("failed to load GGUF: {e}"))?;
    let model = client.completion_model("field-eval");
    Ok(Loaded {
        _client: client,
        model,
    })
}

fn preamble(hits: &[corpus::Hit<'static>]) -> String {
    let mut p = String::from(SYSTEM_RULES);
    p.push_str("\n\nRetrieved manual sections:\n\n");
    p.push_str(&corpus::render_context(hits));
    p
}

/// Direct corpus answer used when no model is loaded.
fn retrieval_answer(hits: &[corpus::Hit<'static>]) -> String {
    if hits.is_empty() {
        return "No matching section in the embedded corpus. Try naming the symptom, the body part, or the vehicle system.".to_string();
    }
    let mut out = String::new();
    for (i, hit) in hits.iter().enumerate() {
        let body = hit.chunk.body.trim();
        let body = if body.chars().count() > 700 {
            format!("{}\u{2026}", body.chars().take(700).collect::<String>())
        } else {
            body.to_string()
        };
        out.push_str(&format!(
            "{}{}\n\n{}\n\n",
            if i == 0 { "" } else { "Also: " },
            hit.chunk.citation(),
            body
        ));
    }
    out.push_str("[retrieval only \u{2014} load a GGUF model for synthesized answers]");
    out
}

fn history_from(turns: &[tui::Turn]) -> Vec<Message> {
    let mut out = Vec::new();
    for turn in turns {
        match turn.role {
            Role::User => out.push(Message::user(turn.text.clone())),
            Role::Assistant if !turn.text.trim().is_empty() => {
                out.push(Message::assistant(turn.text.clone()))
            }
            _ => {}
        }
    }
    // The newest user turn is submitted as the prompt, not as history.
    out.pop();
    out
}

fn spawn_answer(
    loaded: Arc<Loaded>,
    preamble: String,
    history: Vec<Message>,
    prompt: String,
    max_tokens: u64,
    tx: mpsc::UnboundedSender<StreamMsg>,
    cancel: Arc<AtomicBool>,
) {
    tokio::spawn(async move {
        // Holding `loaded` for the whole task keeps the worker alive.
        let agent = AgentBuilder::new(loaded.model.clone())
            .preamble(preamble)
            .max_tokens(max_tokens)
            .temperature(0.2)
            .build();
        let mut stream = agent.prompt(prompt.as_str()).history(history).stream();

        while let Some(item) = stream.next().await {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            match item {
                Ok(MultiTurnStreamItem::StreamAssistantItem(Item::Event(StreamEvent::Text {
                    text,
                    ..
                }))) => {
                    if tx.send(StreamMsg::Delta(text)).is_err() {
                        break;
                    }
                }
                Ok(MultiTurnStreamItem::FinalResponse(res)) => {
                    let _ = tx.send(StreamMsg::Final(res.output().to_string()));
                }
                Ok(_) => {}
                Err(err) => {
                    let _ = tx.send(StreamMsg::Error(err.to_string()));
                    break;
                }
            }
        }
    });
}

fn selftest() -> Result<(), String> {
    let cases = [
        ("how do I stop severe bleeding on a leg", "first-aid.md", "Massive bleeding"),
        ("car cranks but won't start", "vehicle.md", "Engine cranks but will not start"),
        ("purify water in the field", "survival.md", "Water procurement"),
        ("signs of hypothermia", "survival.md", "Cold injury"),
        ("adult cpr compression rate", "first-aid.md", "CPR"),
        ("brake pedal feels spongy", "vehicle.md", "Brakes"),
    ];
    let mut failures = 0;
    for (query, want_doc, want_heading) in cases {
        let hits = corpus::search(query, TOP_K);
        let ok = hits
            .first()
            .map(|h| h.chunk.doc == want_doc && h.chunk.heading == want_heading)
            .unwrap_or(false);
        if ok {
            println!("PASS  {query:45} -> {}", hits[0].chunk.citation());
        } else {
            failures += 1;
            println!(
                "FAIL  {query:45} -> {:?}",
                hits.first().map(|h| h.chunk.citation())
            );
        }
    }
    println!(
        "\ncorpus: {} docs, {} sections",
        corpus::DOCS.len(),
        corpus::index().chunks.len()
    );
    if failures == 0 {
        println!("selftest: OK");
        Ok(())
    } else {
        Err(format!("{failures} retrieval case(s) failed"))
    }
}

async fn ask_once(model: Option<Arc<Loaded>>, question: &str, app: &mut App) {
    let hits = corpus::search(question, TOP_K);
    app.sources = hits.iter().map(|h| h.chunk.citation()).collect();
    app.push(Role::User, question);
    app.push(Role::Assistant, "");
    println!("\n\x1b[2m> {question}\x1b[0m");

    match model {
        Some(model) => {
            let (tx, mut rx) = mpsc::unbounded_channel();
            let cancel = Arc::new(AtomicBool::new(false));
            spawn_answer(
                model,
                preamble(&hits),
                history_from(&app.turns),
                question.to_string(),
                ASK_MAX_TOKENS,
                tx,
                cancel,
            );
            let started = std::time::Instant::now();
            let mut printed = false;
            while let Some(msg) = rx.recv().await {
                match msg {
                    StreamMsg::Delta(text) => {
                        if !printed {
                            println!();
                            printed = true;
                        }
                        print!("{text}");
                        let _ = std::io::Write::flush(&mut std::io::stdout());
                        app.delta(&text);
                    }
                    StreamMsg::Final(text) => {
                        if !printed && !text.trim().is_empty() {
                            println!("\n{text}");
                        }
                        app.finalize(&text);
                        break;
                    }
                    StreamMsg::Error(err) => {
                        eprintln!("\ninference error: {err}");
                        break;
                    }
                }
            }
            println!(
                "\n\x1b[2m[{}s \u{00b7} {} section(s) retrieved]\x1b[0m",
                started.elapsed().as_secs(),
                hits.len()
            );
        }
        None => {
            let answer = retrieval_answer(&hits);
            println!("\n{answer}");
            app.finalize(&answer);
        }
    }
    app.busy = false;
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let args = parse_args()?;

    if args.selftest {
        return selftest();
    }

    let model: Option<Arc<Loaded>> = match args.model.clone() {
        Some(path) => {
            let path_for_task = path.clone();
            let ctx = args.ctx;
            match tokio::task::spawn_blocking(move || load_model(&path_for_task, ctx)).await {
            Ok(Ok(loaded)) => Some(Arc::new(loaded)),
            Ok(Err(err)) => {
                eprintln!("warning: {err}\ncontinuing without a model (retrieval only)");
                None
            }
            Err(err) => {
                eprintln!("warning: model load task failed: {err}");
                None
            }
            }
        }
        None => {
            eprintln!("no --model given; retrieval-only mode (FIELD_EVAL_MODEL is unset)");
            None
        }
    };

    let model_label = args
        .model
        .as_deref()
        .and_then(|p| std::path::Path::new(p).file_name())
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| "none".to_string());

    let mut app = App::new(model_label, model.is_none());

    if let Some(question) = args.ask {
        ask_once(model, &question, &mut app).await;
        return Ok(());
    }

    let mut tui = Tui::enter().map_err(|e| e.to_string())?;
    let mut rx: Option<mpsc::UnboundedReceiver<StreamMsg>> = None;
    let mut cancel = Arc::new(AtomicBool::new(false));

    loop {
        if app.quit {
            break;
        }

        if let Some(channel) = rx.as_mut() {
            loop {
                match channel.try_recv() {
                    Ok(StreamMsg::Delta(text)) => app.delta(&text),
                    Ok(StreamMsg::Final(text)) => {
                        app.finalize(&text);
                        app.busy = false;
                    }
                    Ok(StreamMsg::Error(err)) => {
                        app.push(Role::System, format!("inference error: {err}"));
                        app.busy = false;
                    }
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        if app.busy {
                            app.busy = false;
                        }
                        break;
                    }
                }
            }
        }

        tui.draw(&mut app).map_err(|e| e.to_string())?;

        let Some(key) = tui::poll_key(Duration::from_millis(50)).map_err(|e| e.to_string())?
        else {
            continue;
        };

        match tui::handle_key(&mut app, key) {
            Action::Quit => {
                app.quit = true;
            }
            Action::Cancel => {
                cancel.store(true, Ordering::Relaxed);
                app.busy = false;
                app.push(Role::System, "generation cancelled");
            }
            Action::Submit(question) => {
                let hits = corpus::search(&question, TOP_K);
                app.sources = hits.iter().map(|h| h.chunk.citation()).collect();
                app.push(Role::User, &question);
                app.push(Role::Assistant, "");
                app.busy = true;
                cancel = Arc::new(AtomicBool::new(false));

                match model.clone() {
                    Some(model) => {
                        let (tx, new_rx) = mpsc::unbounded_channel();
                        spawn_answer(
                            model,
                            preamble(&hits),
                            history_from(&app.turns),
                            question,
                            MAX_TOKENS,
                            tx,
                            cancel.clone(),
                        );
                        rx = Some(new_rx);
                    }
                    None => {
                        app.finalize(&retrieval_answer(&hits));
                        app.busy = false;
                    }
                }
            }
            Action::None => {}
        }
    }

    tui.exit().map_err(|e| e.to_string())?;
    Ok(())
}
