//! The thread that does the work, and the two channels that reach it.
//!
//! egui redraws the window on one thread and expects each frame to take a
//! millisecond or two. A scan takes seconds and a search on a million files
//! takes tens of milliseconds, so neither may happen on that thread: the
//! window's job is to send a request and to draw whatever has come back.
//!
//! The requests are collapsed before they are run. Dragging the scrollbar can
//! queue forty pages and a fast typist can queue six searches, and only the
//! last of each is still wanted — running the rest would make the window feel
//! slower the harder it was being used.

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

use eframe::egui;
use ferret_shell::watch::WatchEvent;
use ferret_shell::{
    engine, AppState, IndexUpdate, Row, ScanOptions, SearchArgs, SearchResponse, VolumeInfo,
};

/// What the window asks for.
#[derive(Debug)]
pub enum Request {
    ListVolumes,
    Scan(char),
    Search { args: SearchArgs, token: u64 },
    Page { offset: usize, limit: usize, token: u64 },
    Open(String),
    Reveal(String),
    /// Whether this process can read a raw volume, asked only after a scan has
    /// already failed.
    CheckElevation,
    RestartElevated,
}

/// What comes back.
#[derive(Debug)]
pub enum Event {
    Volumes(Vec<VolumeInfo>),
    ScanProgress { letter: char, percent: u8 },
    Scanned(char, Result<VolumeInfo, String>),
    Searched {
        token: u64,
        result: Result<SearchResponse, String>,
    },
    Paged {
        token: u64,
        offset: usize,
        rows: Vec<Row>,
    },
    IndexUpdated(IndexUpdate),
    IndexStale,
    Elevation(bool),
    /// A shell action failed, as a `code:detail` the window can phrase.
    Failed(String),
    /// An elevated copy is starting, so this one should close.
    Restarting,
}

pub struct Worker {
    requests: Sender<Request>,
    events: Receiver<Event>,
}

impl Worker {
    /// Start the worker. It owns the indexes; the window never touches them.
    ///
    /// The context is cloned in so that anything arriving from the worker — a
    /// scan that finished, a journal batch, a page of rows — wakes the window up
    /// instead of waiting for the next mouse move.
    pub fn start(ctx: egui::Context) -> Self {
        let (request_tx, request_rx) = mpsc::channel::<Request>();
        let (event_tx, event_rx) = mpsc::channel::<Event>();

        std::thread::Builder::new()
            .name("ferret-worker".into())
            .spawn(move || run(request_rx, event_tx, ctx))
            .expect("could not start the Ferret worker thread");

        Self {
            requests: request_tx,
            events: event_rx,
        }
    }

    /// Ask for something. Silently dropped if the worker has gone, which only
    /// happens while the process is already closing.
    pub fn send(&self, request: Request) {
        let _ = self.requests.send(request);
    }

    /// Everything that has arrived since the last frame.
    pub fn drain(&self) -> Vec<Event> {
        let mut events = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            events.push(event);
        }
        events
    }
}

fn run(requests: Receiver<Request>, events: Sender<Event>, ctx: egui::Context) {
    let state = AppState::default();
    let sink = Sink { events, ctx };

    loop {
        // Block until there is something to do, then take everything else that
        // is already waiting so the batch can be collapsed.
        let first = match requests.recv() {
            Ok(request) => request,
            Err(_) => return, // the window has closed
        };
        let mut batch = vec![first];
        loop {
            match requests.try_recv() {
                Ok(request) => batch.push(request),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        for request in collapse(batch) {
            if !handle(request, &state, &sink) {
                return;
            }
        }
    }
}

/// Drop the requests that a later one has already made pointless.
///
/// Only the newest search and the newest page are still wanted. A search also
/// retires every page in the batch, because those offsets were into a hit list
/// that is about to be replaced.
fn collapse(batch: Vec<Request>) -> Vec<Request> {
    let has_search = batch
        .iter()
        .any(|request| matches!(request, Request::Search { .. }));

    let mut seen_search = false;
    let mut seen_page = false;
    let mut kept = Vec::with_capacity(batch.len());

    // Backwards, so "the first one seen" is the newest one sent.
    for request in batch.into_iter().rev() {
        let keep = match &request {
            Request::Search { .. } => !std::mem::replace(&mut seen_search, true),
            Request::Page { .. } => !has_search && !std::mem::replace(&mut seen_page, true),
            _ => true,
        };
        if keep {
            kept.push(request);
        }
    }
    kept.reverse();
    kept
}

/// Run one request. Returns false when the worker should stop.
fn handle(request: Request, state: &AppState, sink: &Sink) -> bool {
    match request {
        Request::ListVolumes => sink.send(Event::Volumes(engine::list_volumes(state))),

        Request::Scan(letter) => {
            let result = engine::scan(state, letter, ScanOptions::default(), |progress| {
                sink.send(Event::ScanProgress {
                    letter,
                    percent: progress.percent,
                });
            });

            if result.is_ok() {
                // Any watcher from a previous scan is looking at an index that
                // has just been replaced.
                ferret_shell::watch::retire_all();
                let watch_sink = sink.clone();
                ferret_shell::watch::spawn(state.clone(), letter, move |event| match event {
                    WatchEvent::Updated(update) => watch_sink.send(Event::IndexUpdated(update)),
                    WatchEvent::Stale(_) => watch_sink.send(Event::IndexStale),
                });
            }
            sink.send(Event::Scanned(letter, result));
        }

        Request::Search { args, token } => sink.send(Event::Searched {
            token,
            result: engine::search(state, &args),
        }),

        Request::Page {
            offset,
            limit,
            token,
        } => sink.send(Event::Paged {
            token,
            offset,
            rows: engine::page(state, offset, limit),
        }),

        Request::Open(path) => {
            if let Err(err) = engine::open_path(&path) {
                sink.send(Event::Failed(err));
            }
        }

        Request::Reveal(path) => {
            if let Err(err) = engine::reveal_path(&path) {
                sink.send(Event::Failed(err));
            }
        }

        Request::CheckElevation => sink.send(Event::Elevation(engine::is_elevated())),

        Request::RestartElevated => match engine::restart_elevated() {
            Ok(()) => {
                sink.send(Event::Restarting);
                return false;
            }
            Err(err) => sink.send(Event::Failed(err)),
        },
    }
    true
}

/// The reply half, with the repaint attached.
///
/// Every event is followed by a repaint request, because the window is otherwise
/// asleep: egui only redraws when something happens, and from its point of view
/// a worker finishing is not an input event.
#[derive(Clone)]
struct Sink {
    events: Sender<Event>,
    ctx: egui::Context,
}

impl Sink {
    fn send(&self, event: Event) {
        if self.events.send(event).is_ok() {
            self.ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn search(token: u64) -> Request {
        Request::Search {
            args: SearchArgs::default(),
            token,
        }
    }

    fn page(offset: usize) -> Request {
        Request::Page {
            offset,
            limit: 400,
            token: 1,
        }
    }

    fn describe(requests: &[Request]) -> Vec<String> {
        requests
            .iter()
            .map(|request| match request {
                Request::Search { token, .. } => format!("search{token}"),
                Request::Page { offset, .. } => format!("page{offset}"),
                Request::Open(path) => format!("open:{path}"),
                Request::Scan(letter) => format!("scan{letter}"),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn only_the_newest_search_survives() {
        let kept = collapse(vec![search(1), search(2), search(3)]);
        assert_eq!(describe(&kept), ["search3"]);
    }

    #[test]
    fn only_the_newest_page_survives() {
        // Forty pages queued by one drag of the scrollbar.
        let kept = collapse((0..40).map(|n| page(n * 400)).collect());
        assert_eq!(describe(&kept), ["page15600"]);
    }

    #[test]
    fn a_search_retires_every_page_queued_beside_it() {
        // Those offsets point into a hit list that is about to be replaced, so
        // running them would fill the table with rows from the old query.
        let kept = collapse(vec![page(0), search(1), page(400)]);
        assert_eq!(describe(&kept), ["search1"]);
    }

    #[test]
    fn everything_else_is_kept_in_the_order_it_was_asked_for() {
        let kept = collapse(vec![
            Request::Open("a".into()),
            Request::Scan('C'),
            Request::Open("b".into()),
        ]);
        assert_eq!(describe(&kept), ["open:a", "scanC", "open:b"]);
    }

    #[test]
    fn collapsing_keeps_a_survivor_in_its_original_position() {
        // The scan must still run before the search that followed it.
        let kept = collapse(vec![Request::Scan('C'), search(1), search(2)]);
        assert_eq!(describe(&kept), ["scanC", "search2"]);
    }

    #[test]
    fn a_single_request_passes_through_untouched() {
        assert_eq!(describe(&collapse(vec![page(800)])), ["page800"]);
        assert!(collapse(Vec::new()).is_empty());
    }
}
