mod activity;
mod api;
mod app;
mod config;
mod headers;
mod icons;
mod input;
mod markdown;
mod switcher;
mod text;
mod toolview;
mod ui;
mod ws;

use std::{io::stdout, thread, time::Duration};

use clap::Parser;
use ratatui::{
    DefaultTerminal,
    crossterm::{
        event::{
            self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste,
            EnableMouseCapture, Event, KeyEventKind, MouseEventKind,
        },
        execute,
    },
};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use crate::app::{App, Msg};

#[derive(Debug, Parser)]
#[command(
    name = "iowb-tui",
    version,
    about = "Terminal client for an io-workbench server"
)]
struct Cli {
    /// Server address, for example http://127.0.0.1:8100. Defaults to the one used last time.
    #[arg(long, env = "IO_WORKBENCH_URL")]
    server: Option<String>,
}

/// Turns bracketed paste and mouse capture back off however the program ends,
/// even on a panic.
struct InputModes;

impl InputModes {
    fn enable() -> Self {
        let _ = execute!(stdout(), EnableBracketedPaste, EnableMouseCapture);
        Self
    }
}

impl Drop for InputModes {
    fn drop(&mut self) {
        let _ = execute!(stdout(), DisableMouseCapture, DisableBracketedPaste);
    }
}

pub fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let mut terminal = ratatui::init();
        let modes = InputModes::enable();
        let result = event_loop(&mut terminal, cli).await;
        drop(modes);
        ratatui::restore();
        result
    })
}

/// Terminal input is read on its own thread so a slow redraw never drops keys.
fn spawn_input_reader(tx: UnboundedSender<Msg>) {
    thread::spawn(move || {
        loop {
            match event::poll(Duration::from_millis(250)) {
                Ok(true) => {
                    let msg = match event::read() {
                        Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => Msg::Key(key),
                        Ok(Event::Paste(text)) => Msg::Paste(text),
                        Ok(Event::Mouse(mouse)) => match mouse.kind {
                            MouseEventKind::ScrollUp => Msg::Wheel { up: true },
                            MouseEventKind::ScrollDown => Msg::Wheel { up: false },
                            _ => continue,
                        },
                        // A resize needs a redraw; a tick triggers one.
                        Ok(Event::Resize(..)) => Msg::Tick,
                        Ok(_) => continue,
                        Err(_) => return,
                    };
                    if tx.send(msg).is_err() {
                        return;
                    }
                }
                Ok(false) if tx.is_closed() => return,
                Ok(false) => {}
                Err(_) => return,
            }
        }
    });
}

async fn event_loop(terminal: &mut DefaultTerminal, cli: Cli) -> anyhow::Result<()> {
    let (tx, mut rx) = unbounded_channel::<Msg>();
    spawn_input_reader(tx.clone());
    let mut app = App::new(tx, config::load(), cli.server, true);
    app.start();
    let mut ticker = tokio::time::interval(Duration::from_millis(250));
    while !app.quit {
        terminal.draw(|frame| ui::draw(frame, &app))?;
        tokio::select! {
            Some(msg) = rx.recv() => {
                app.update(msg);
                while let Ok(more) = rx.try_recv() {
                    app.update(more);
                }
            }
            _ = ticker.tick() => app.update(Msg::Tick),
        }
    }
    Ok(())
}
