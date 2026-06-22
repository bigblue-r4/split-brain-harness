use anyhow::Result;
use crossterm::{
    event::{self, Event as CEvent, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Wrap},
    Frame, Terminal,
};
use serde::{Deserialize, Serialize};
use split_brain_harness::{analyze, config::build_config, types::HarnessResult};
use std::{io::stdout, time::Duration};
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Phase {
    Idle,
    Analyzing,
    Streaming,
}

#[derive(Debug, Clone, PartialEq)]
enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone)]
struct ChatMessage {
    role: Role,
    content: String,
}

struct App {
    messages: Vec<ChatMessage>,
    telemetry: Option<HarnessResult>,
    input: String,
    streaming_buf: String,
    phase: Phase,
    analysis_model: String,
    chat_model: String,
    ollama_endpoint: String,
    verify_mode: String,
    show_help: bool,
}

// ---------------------------------------------------------------------------
// Background events
// ---------------------------------------------------------------------------

enum AppEvent {
    Telemetry(Box<HarnessResult>),
    ChatToken(String),
    ChatDone,
    Error(String),
}

// ---------------------------------------------------------------------------
// Ollama /api/chat types
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
struct OllamaMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct OllamaChatRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    stream: bool,
}

#[derive(Deserialize)]
struct OllamaChatChunk {
    #[serde(default, rename = "done")]
    _done: bool,
    #[serde(default)]
    message: OllamaChunkMsg,
}

#[derive(Deserialize, Default)]
struct OllamaChunkMsg {
    #[serde(default)]
    content: String,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let config = build_config();
    let chat_model = std::env::var("SBH_CHAT_MODEL").unwrap_or_else(|_| config.model_name.clone());
    let ollama_endpoint = config.endpoint.clone();
    let verify_mode = config.verify_mode.to_string();

    let app = App {
        messages: Vec::new(),
        telemetry: None,
        input: String::new(),
        streaming_buf: String::new(),
        phase: Phase::Idle,
        analysis_model: config.model_name.clone(),
        chat_model,
        ollama_endpoint,
        verify_mode,
        show_help: false,
    };

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(out))?;

    let result = run(&mut terminal, app, config).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

// ---------------------------------------------------------------------------
// Main event loop
// ---------------------------------------------------------------------------

async fn run(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    mut app: App,
    config: split_brain_harness::types::Config,
) -> Result<()> {
    let (tx, mut rx) = mpsc::channel::<AppEvent>(128);

    loop {
        terminal.draw(|f| ui(f, &app))?;

        // Drain background task messages (non-blocking)
        while let Ok(ev) = rx.try_recv() {
            match ev {
                AppEvent::Telemetry(t) => {
                    app.telemetry = Some(*t);
                    app.phase = Phase::Streaming;
                }
                AppEvent::ChatToken(tok) => {
                    app.streaming_buf.push_str(&tok);
                }
                AppEvent::ChatDone => {
                    let content = std::mem::take(&mut app.streaming_buf);
                    app.messages.push(ChatMessage {
                        role: Role::Assistant,
                        content,
                    });
                    app.phase = Phase::Idle;
                }
                AppEvent::Error(e) => {
                    app.messages.push(ChatMessage {
                        role: Role::System,
                        content: e,
                    });
                    app.streaming_buf.clear();
                    app.phase = Phase::Idle;
                }
            }
        }

        if !event::poll(Duration::from_millis(50))? {
            continue;
        }

        if let CEvent::Key(key) = event::read()? {
            match (key.code, key.modifiers) {
                (KeyCode::Char('c'), KeyModifiers::CONTROL) | (KeyCode::Esc, _) => {
                    if app.show_help {
                        app.show_help = false;
                    } else {
                        break;
                    }
                }

                (KeyCode::Char('?'), _) if app.phase == Phase::Idle => {
                    app.show_help = !app.show_help;
                }

                (KeyCode::Enter, _) if app.phase == Phase::Idle && !app.input.is_empty() => {
                    let input = std::mem::take(&mut app.input);
                    app.show_help = false;

                    // Handle /clear command
                    if input.trim() == "/clear" {
                        app.messages.clear();
                        app.telemetry = None;
                        continue;
                    }

                    app.messages.push(ChatMessage {
                        role: Role::User,
                        content: input.clone(),
                    });
                    app.phase = Phase::Analyzing;

                    let history: Vec<OllamaMessage> = app
                        .messages
                        .iter()
                        .filter_map(|m| match m.role {
                            Role::User => Some(OllamaMessage {
                                role: "user".into(),
                                content: m.content.clone(),
                            }),
                            Role::Assistant => Some(OllamaMessage {
                                role: "assistant".into(),
                                content: m.content.clone(),
                            }),
                            Role::System => None,
                        })
                        .collect();

                    let tx2 = tx.clone();
                    let cfg = config.clone();
                    let chat_model = app.chat_model.clone();
                    let endpoint = app.ollama_endpoint.clone();

                    tokio::spawn(async move {
                        match analyze(&input, &cfg).await {
                            Ok(r) => {
                                let _ = tx2.send(AppEvent::Telemetry(Box::new(r))).await;
                            }
                            Err(e) => {
                                let _ = tx2.send(AppEvent::Error(e.to_string())).await;
                                return;
                            }
                        }
                        if let Err(e) =
                            stream_chat(tx2.clone(), endpoint, chat_model, history).await
                        {
                            let _ = tx2.send(AppEvent::Error(e.to_string())).await;
                        }
                    });
                }

                (KeyCode::Char(c), _) if app.phase == Phase::Idle => {
                    app.input.push(c);
                }
                (KeyCode::Backspace, _) if app.phase == Phase::Idle => {
                    app.input.pop();
                }

                _ => {}
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Ollama streaming chat
// ---------------------------------------------------------------------------

async fn stream_chat(
    tx: mpsc::Sender<AppEvent>,
    endpoint: String,
    model: String,
    messages: Vec<OllamaMessage>,
) -> Result<()> {
    let client = reqwest::Client::new();
    let mut resp = client
        .post(format!("{endpoint}/api/chat"))
        .json(&OllamaChatRequest {
            model,
            messages,
            stream: true,
        })
        .send()
        .await?;

    let mut buf = String::new();
    while let Some(chunk) = resp.chunk().await? {
        buf.push_str(std::str::from_utf8(&chunk)?);
        while let Some(nl) = buf.find('\n') {
            let line = buf[..nl].trim().to_string();
            buf.drain(..=nl);
            if line.is_empty() {
                continue;
            }
            if let Ok(parsed) = serde_json::from_str::<OllamaChatChunk>(&line) {
                if !parsed.message.content.is_empty() {
                    tx.send(AppEvent::ChatToken(parsed.message.content)).await?;
                }
            }
        }
    }

    tx.send(AppEvent::ChatDone).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// UI rendering
// ---------------------------------------------------------------------------

fn ui(frame: &mut Frame, app: &App) {
    let root = Layout::vertical([
        Constraint::Length(1), // title bar
        Constraint::Min(0),    // chat + telemetry
        Constraint::Length(1), // status bar
        Constraint::Length(3), // input
    ])
    .split(frame.area());

    render_title(frame, root[0]);

    let cols =
        Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)]).split(root[1]);
    render_chat(frame, app, cols[0]);
    render_telemetry(frame, app, cols[1]);

    render_status(frame, app, root[2]);
    render_input(frame, app, root[3]);

    if app.show_help {
        render_help(frame);
    }
}

fn render_title(frame: &mut Frame, area: Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " SGAIL Labs",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " ◆ Split-Brain Monitor",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ])),
        area,
    );
}

fn render_chat(frame: &mut Frame, app: &App, area: Rect) {
    let inner_h = area.height.saturating_sub(2) as usize;
    let mut lines: Vec<Line> = Vec::new();

    for msg in &app.messages {
        let (prefix, color) = match msg.role {
            Role::User => ("  you  ", Color::Cyan),
            Role::Assistant => ("  sbh  ", Color::Green),
            Role::System => ("  err  ", Color::Red),
        };
        let mut chunks = msg.content.lines();
        if let Some(first) = chunks.next() {
            lines.push(Line::from(vec![
                Span::styled(
                    prefix,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::raw(first.to_string()),
            ]));
            for rest in chunks {
                lines.push(Line::from(vec![
                    Span::raw("         "),
                    Span::raw(rest.to_string()),
                ]));
            }
        }
        if msg.role != Role::User {
            lines.push(Line::from(""));
        }
    }

    if app.phase == Phase::Streaming || !app.streaming_buf.is_empty() {
        let mut chunks = app.streaming_buf.lines();
        if let Some(first) = chunks.next() {
            lines.push(Line::from(vec![
                Span::styled(
                    "  sbh  ",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(first.to_string()),
                Span::styled("▌", Style::default().fg(Color::DarkGray)),
            ]));
            for rest in chunks {
                lines.push(Line::from(vec![
                    Span::raw("         "),
                    Span::raw(rest.to_string()),
                ]));
            }
        } else {
            lines.push(Line::from(vec![
                Span::styled(
                    "  sbh  ",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("▌", Style::default().fg(Color::DarkGray)),
            ]));
        }
    } else if app.phase == Phase::Analyzing {
        lines.push(Line::from(vec![
            Span::styled("  sbh  ", Style::default().fg(Color::DarkGray)),
            Span::styled("analyzing…", Style::default().fg(Color::DarkGray)),
        ]));
    }

    let scroll = lines.len().saturating_sub(inner_h) as u16;

    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::bordered()
                    .title(" Chat ")
                    .border_style(Style::default().fg(Color::DarkGray)),
            )
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        area,
    );
}

fn render_telemetry(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered()
        .title(" Telemetry ")
        .border_style(Style::default().fg(Color::DarkGray));

    let lines: Vec<Line> = match &app.telemetry {
        None => vec![
            Line::from(""),
            Line::from(Span::styled(
                "  awaiting input…",
                Style::default().fg(Color::DarkGray),
            )),
        ],
        Some(r) => {
            let risk = r.telemetry.intent_matrix.manipulation_risk.as_str();
            let (risk_color, risk_label) = match risk {
                "high" => (Color::Red, " HIGH ⚠"),
                "medium" => (Color::Yellow, " MED  ▲"),
                _ => (Color::Green, " LOW  ✓"),
            };
            let tone = r.telemetry.affective_telemetry.structural_tone.join(", ");
            let conf = r.verification.confidence;
            let conf_color = if conf > 0.7 {
                Color::Green
            } else if conf > 0.4 {
                Color::Yellow
            } else {
                Color::Red
            };

            let mut lines: Vec<Line> = vec![
                Line::from(""),
                kv(
                    "  emotion  ",
                    &r.telemetry.affective_telemetry.primary_emotion,
                    Color::White,
                ),
                kv(
                    "  intensity",
                    &f2(&r.telemetry.affective_telemetry.emotional_intensity),
                    Color::White,
                ),
                kv(
                    "  urgency  ",
                    &f2(&r.telemetry.cognitive_state.urgency_vector),
                    Color::White,
                ),
                kv(
                    "  coherence",
                    &f2(&r.telemetry.cognitive_state.coherence_rating),
                    Color::White,
                ),
                kv("  tone     ", &tone, Color::White),
                Line::from(""),
                kv(
                    "  objective",
                    &r.telemetry.intent_matrix.stated_objective,
                    Color::White,
                ),
                kv(
                    "  subtext  ",
                    &r.telemetry.intent_matrix.subtextual_motive,
                    Color::White,
                ),
                Line::from(""),
                Line::from(vec![
                    Span::styled("  risk     ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        risk_label,
                        Style::default().fg(risk_color).add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(""),
            ];

            let active_packs: Vec<String> = r
                .trace
                .iter()
                .filter(|e| e.stage == "context_injection")
                .filter_map(|e| e.claim.split(": ").nth(1).map(|s| s.to_string()))
                .flat_map(|s| s.split(", ").map(|p| p.to_string()).collect::<Vec<_>>())
                .collect();

            if !active_packs.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  packs",
                    Style::default().fg(Color::DarkGray),
                )));
                for p in &active_packs {
                    lines.push(Line::from(vec![
                        Span::styled("  ⚡ ", Style::default().fg(Color::Yellow)),
                        Span::raw(p.clone()),
                    ]));
                }
                lines.push(Line::from(""));
            }

            if !r.verification.consistency_flags.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  flags",
                    Style::default().fg(Color::DarkGray),
                )));
                for flag in &r.verification.consistency_flags {
                    lines.push(Line::from(vec![
                        Span::styled("  ▸ ", Style::default().fg(Color::Red)),
                        Span::raw(flag.clone()),
                    ]));
                }
                lines.push(Line::from(""));
            }

            lines.push(kv("  confidence", &f2(&conf), conf_color));

            if r.verification.stop_and_ask {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "  ⚠  stop and ask",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                )));
            }

            lines
        }
    };

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_status(frame: &mut Frame, app: &App, area: Rect) {
    let phase = match app.phase {
        Phase::Idle => "idle",
        Phase::Analyzing => "analyzing",
        Phase::Streaming => "streaming",
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " sbh",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  analysis:", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!(" {} ", app.analysis_model),
                Style::default().fg(Color::White),
            ),
            Span::styled(" chat:", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!(" {} ", app.chat_model),
                Style::default().fg(Color::White),
            ),
            Span::styled(" verify:", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!(" {} ", app.verify_mode),
                Style::default().fg(Color::White),
            ),
            Span::styled(" endpoint:", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!(" {} ", app.ollama_endpoint),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(format!(" [{phase}]"), Style::default().fg(Color::DarkGray)),
        ])),
        area,
    );
}

fn render_input(frame: &mut Frame, app: &App, area: Rect) {
    let active = app.phase == Phase::Idle;
    let prompt = if active {
        format!(" > {}▌", app.input)
    } else {
        format!(" > {}", app.input)
    };
    let title = if active {
        " Input  [enter] send  [?] help  [esc] quit "
    } else {
        " Input  [processing…] "
    };
    frame.render_widget(
        Paragraph::new(prompt).block(
            Block::bordered()
                .title(title)
                .border_style(Style::default().fg(if active {
                    Color::Cyan
                } else {
                    Color::DarkGray
                })),
        ),
        area,
    );
}

fn render_help(frame: &mut Frame) {
    let area = frame.area();
    let w = area.width.min(44);
    let h = 12u16;
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect::new(x, y, w, h);

    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::from(vec![
                Span::styled("  Enter     ", Style::default().fg(Color::Cyan)),
                Span::raw("send message"),
            ]),
            Line::from(vec![
                Span::styled("  Backspace ", Style::default().fg(Color::Cyan)),
                Span::raw("delete character"),
            ]),
            Line::from(vec![
                Span::styled("  ?         ", Style::default().fg(Color::Cyan)),
                Span::raw("toggle this help"),
            ]),
            Line::from(vec![
                Span::styled("  Esc       ", Style::default().fg(Color::Cyan)),
                Span::raw("close help / quit"),
            ]),
            Line::from(vec![
                Span::styled("  Ctrl-C    ", Style::default().fg(Color::Cyan)),
                Span::raw("quit"),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("  /clear    ", Style::default().fg(Color::Yellow)),
                Span::raw("clear chat and telemetry"),
            ]),
            Line::from(""),
        ])
        .block(
            Block::bordered()
                .title(" Help ")
                .border_style(Style::default().fg(Color::Cyan)),
        ),
        popup,
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn kv(label: &str, value: &str, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(label.to_string(), Style::default().fg(Color::DarkGray)),
        Span::styled(value.to_string(), Style::default().fg(color)),
    ])
}

fn f2(v: &f32) -> String {
    format!("{v:.2}")
}
