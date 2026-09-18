//! The window.
//!
//! Three things here are worth knowing before reading the rest.
//!
//! 1. *Nothing blocks.* Every call into the engine goes to [`crate::worker`] and
//!    comes back as an event. A frame that waited on a scan would freeze the
//!    window for seconds, and one that waited on a search would stutter on every
//!    keystroke.
//!
//! 2. *The table is virtual twice over.* egui only draws the rows on screen, and
//!    only those rows are ever fetched: the worker keeps the hit list from the
//!    last query, so scrolling asks for a slice rather than searching again. A
//!    query can match a million files and the window holds a thousand.
//!
//! 3. *Errors arrive as codes.* `ferret-shell` never sends a sentence, because
//!    the window can be switched between Turkish and English at any moment. The
//!    wording happens in [`crate::i18n`], at the moment of drawing.

use std::time::{Duration, Instant};

use eframe::egui;
use egui_extras::{Column, TableBuilder};
use ferret_shell::{IndexUpdate, Row, SearchArgs, SearchResponse, VolumeInfo};

use crate::i18n::Lang;
use crate::prefs::{self, Prefs};
use crate::theme::{self, Theme, ROW_HEIGHT};
use crate::worker::{Event, Request, Worker};

/// Typing settles for this long before a search runs.
const DEBOUNCE: Duration = Duration::from_millis(90);

/// How many rows one request to the worker covers.
const WINDOW: usize = 1_000;

/// A new window is fetched once the view comes within this many rows of the edge
/// of the one in hand, which is what keeps ordinary scrolling free of gaps.
const PREFETCH: usize = 150;

/// The size filter's choices, as (bytes, label).
const SIZE_STEPS: &[(u64, &str)] = &[
    (1_048_576, "1 MB"),
    (10_485_760, "10 MB"),
    (104_857_600, "100 MB"),
    (1_073_741_824, "1 GB"),
];

/// The id of the search box, so that keyboard handling can ask whether the
/// caret is in it before deciding what Home and End mean.
const QUERY_ID: &str = "ferret-query";

/// What the middle of the window is showing.
enum Phase {
    /// Before the first volume list has come back.
    Starting,
    Scanning { letter: char, percent: u8 },
    Ready,
    NoVolume,
    /// A scan failed for a reason that is not a permission problem.
    ScanFailed { letter: char, message: String },
    /// A scan failed because the process cannot read a raw volume.
    NeedsElevation,
}

/// A one-line message under the toolbar.
struct Notice {
    text: String,
    /// Warnings are in the danger colour. A confirmation is not a warning, and
    /// colouring "Path copied" red would read as a failure.
    warning: bool,
}

/// The last query's answer, and the slice of it currently in memory.
#[derive(Default)]
struct Results {
    total: usize,
    took_ms: Option<f64>,
    total_size: String,
    sort_skipped: bool,
    /// The cache covers `cache_start .. cache_start + rows.len()`.
    cache_start: usize,
    rows: Vec<Row>,
    /// What to highlight in a name: the part of the query the engine actually
    /// compared against file names.
    needle: String,
    selected: Option<usize>,
}

impl Results {
    fn row(&self, index: usize) -> Option<&Row> {
        index
            .checked_sub(self.cache_start)
            .and_then(|offset| self.rows.get(offset))
    }
}

pub struct Ferret {
    worker: Worker,

    lang: Lang,
    theme: Theme,

    query: String,
    /// When the last keystroke landed, or `None` when nothing is pending.
    typing_since: Option<Instant>,
    /// Bumped per search. Answers arriving with an older token are discarded,
    /// and it doubles as the epoch for page requests.
    token: u64,
    /// The window asked for, so the same one is not asked for every frame.
    pending_window: Option<(usize, usize)>,

    volumes: Vec<VolumeInfo>,
    letter: String,
    volume: Option<VolumeInfo>,
    /// Counts as the change journal has them, which drift from the scan's.
    live_files: u64,
    live_dirs: u64,

    results: Results,
    /// Set when the selection moved by keyboard, so the table can follow it.
    scroll_to: Option<usize>,

    sort_by: String,
    descending: bool,
    files_only: bool,
    dirs_only: bool,
    skip_hidden: bool,
    min_size: Option<u64>,

    phase: Phase,
    notice: Option<Notice>,
    /// Set when something should put the caret back in the search box.
    focus_query: bool,
    /// The title already sent to Windows, so it is not set every frame.
    title: String,
    /// How many rows fit on screen, measured while drawing. Page Up and Page
    /// Down need it, and they are handled before the table is laid out.
    rows_per_screen: isize,
}

impl Ferret {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        theme::apply_style(&cc.egui_ctx);

        let saved = cc
            .storage
            .and_then(|storage| eframe::get_value::<Prefs>(storage, prefs::KEY))
            .unwrap_or_default();

        theme::apply(&cc.egui_ctx, saved.theme);

        let worker = Worker::start(cc.egui_ctx.clone());
        worker.send(Request::ListVolumes);

        Self {
            worker,
            lang: saved.lang,
            theme: saved.theme,
            query: String::new(),
            typing_since: None,
            token: 0,
            pending_window: None,
            volumes: Vec::new(),
            letter: saved.drive.clone(),
            volume: None,
            live_files: 0,
            live_dirs: 0,
            results: Results::default(),
            scroll_to: None,
            sort_by: saved.sort_by,
            descending: saved.descending,
            files_only: saved.files_only,
            dirs_only: saved.dirs_only,
            skip_hidden: saved.skip_hidden,
            min_size: saved.min_size,
            phase: Phase::Starting,
            notice: None,
            focus_query: true,
            title: String::new(),
            rows_per_screen: 20,
        }
    }

    fn prefs(&self) -> Prefs {
        Prefs {
            lang: self.lang,
            theme: self.theme,
            drive: self.letter.clone(),
            sort_by: self.sort_by.clone(),
            descending: self.descending,
            files_only: self.files_only,
            dirs_only: self.dirs_only,
            skip_hidden: self.skip_hidden,
            min_size: self.min_size,
        }
    }

    fn warn(&mut self, text: String) {
        self.notice = Some(Notice {
            text,
            warning: true,
        });
    }

    fn inform(&mut self, text: String) {
        self.notice = Some(Notice {
            text,
            warning: false,
        });
    }

    fn scanning(&self) -> bool {
        matches!(self.phase, Phase::Scanning { .. } | Phase::Starting)
    }

    /* ---------------------------------------------------------------- *
     * Events from the worker
     * ---------------------------------------------------------------- */

    fn absorb(&mut self, events: Vec<Event>) {
        for event in events {
            match event {
                Event::Volumes(volumes) => self.choose_volume(volumes),

                Event::ScanProgress { letter, percent } => {
                    if let Phase::Scanning { letter: current, .. } = self.phase {
                        if current == letter {
                            self.phase = Phase::Scanning { letter, percent };
                        }
                    }
                }

                Event::Scanned(_letter, Ok(info)) => {
                    self.adopt_volume(info);
                    self.phase = Phase::Ready;
                    self.notice = None;
                    self.search_now();
                }

                Event::Scanned(letter, Err(message)) => {
                    // Reading a raw volume needs administrator rights. Rather
                    // than demand them before the window even opens, Ferret asks
                    // only once it actually needs them — so which screen to show
                    // depends on an answer that has not arrived yet.
                    self.phase = Phase::ScanFailed { letter, message };
                    self.worker.send(Request::CheckElevation);
                }

                Event::Elevation(false) => {
                    // Only ever asked after a scan failed, and only that failure
                    // is worth reinterpreting: the answer is stale by the time
                    // anything else is on screen.
                    if matches!(self.phase, Phase::ScanFailed { .. }) {
                        self.phase = Phase::NeedsElevation;
                    }
                }
                Event::Elevation(true) => {}

                Event::Searched { token, result } => {
                    if token == self.token {
                        self.adopt_results(result);
                    }
                }

                Event::Paged {
                    token,
                    offset,
                    rows,
                } => {
                    // A page from a retired query points into the wrong hit list.
                    if token == self.token {
                        self.results.cache_start = offset;
                        self.results.rows = rows;
                        self.pending_window = None;
                    }
                }

                Event::IndexUpdated(update) => self.absorb_update(update),

                Event::IndexStale => self.warn(self.lang.strings().index_stale.to_string()),

                Event::Failed(code) => self.warn(self.lang.error(&code)),

                Event::Restarting => {}
            }
        }
    }

    /// Pick which drive to open with: the remembered one, else one that is
    /// already indexed, else C, else whatever exists.
    fn choose_volume(&mut self, volumes: Vec<VolumeInfo>) {
        self.volumes = volumes;

        let chosen = self
            .volumes
            .iter()
            .find(|v| v.letter == self.letter)
            .or_else(|| self.volumes.iter().find(|v| v.indexed))
            .or_else(|| self.volumes.iter().find(|v| v.letter == "C"))
            .or_else(|| self.volumes.first())
            .cloned();

        let Some(chosen) = chosen else {
            self.phase = Phase::NoVolume;
            return;
        };

        self.letter = chosen.letter.clone();
        if chosen.indexed {
            self.adopt_volume(chosen);
            self.phase = Phase::Ready;
            self.search_now();
        } else {
            self.start_scan();
        }
    }

    fn adopt_volume(&mut self, info: VolumeInfo) {
        self.letter = info.letter.clone();
        self.live_files = info.files;
        self.live_dirs = info.dirs;
        self.volume = Some(info);
    }

    fn adopt_results(&mut self, result: Result<SearchResponse, String>) {
        match result {
            Ok(response) => {
                self.results.total = response.total;
                self.results.took_ms = Some(response.took_ms);
                self.results.total_size = response.total_size;
                self.results.sort_skipped = response.sort_skipped;
                self.results.cache_start = 0;
                self.results.rows = response.rows;
                self.results.selected = None;
                self.pending_window = None;
                // A new result set means the old scroll position means nothing.
                self.scroll_to = Some(0);

                self.notice = None;
                if response.sort_skipped {
                    self.warn(self.lang.strings().sort_skipped.to_string());
                }
            }
            Err(code) => self.warn(self.lang.error(&code)),
        }
    }

    /// The journal folded new activity into the index.
    ///
    /// The counts refresh always, but the result list is only re-run when the
    /// user is sitting at the top of it: re-sorting the ground under someone who
    /// is scrolling is worse than showing a row a few seconds out of date.
    fn absorb_update(&mut self, update: IndexUpdate) {
        if update.letter != self.letter {
            return;
        }
        self.live_files = update.files;
        self.live_dirs = update.dirs;

        let at_top = self.results.cache_start == 0
            && self.results.selected.unwrap_or(0) < 20
            && self.scroll_to.is_none();
        if matches!(self.phase, Phase::Ready) && at_top {
            self.typing_since = Some(Instant::now());
        }
    }

    /* ---------------------------------------------------------------- *
     * Asking for things
     * ---------------------------------------------------------------- */

    fn start_scan(&mut self) {
        let Some(letter) = self.letter.chars().next() else {
            return;
        };
        self.phase = Phase::Scanning { letter, percent: 0 };
        self.notice = None;
        self.worker.send(Request::Scan(letter));
    }

    /// Run the query now. Every path that changes what the result should be ends
    /// up here, either directly or after the debounce.
    fn search_now(&mut self) {
        self.typing_since = None;
        if self.letter.is_empty() || self.scanning() {
            return;
        }

        // The needle for highlighting is the last path segment, which is what
        // the engine actually compares against the file name.
        self.results.needle = self
            .query
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or_default()
            .trim()
            .to_lowercase();

        self.token += 1;
        self.pending_window = None;
        self.worker.send(Request::Search {
            args: SearchArgs {
                letter: self.letter.clone(),
                text: self.query.clone(),
                sort_by: self.sort_by.clone(),
                descending: self.descending,
                files_only: self.files_only,
                dirs_only: self.dirs_only,
                skip_hidden: self.skip_hidden,
                min_size: self.min_size,
                limit: Some(WINDOW),
            },
            token: self.token,
        });
    }

    fn search_soon(&mut self) {
        self.typing_since = Some(Instant::now());
    }

    /// Fetch the rows around what is on screen, if the ones in hand no longer
    /// reach far enough.
    fn ensure_window(&mut self, first: usize, last: usize) {
        if self.results.total == 0 {
            return;
        }
        let last = last.min(self.results.total - 1);
        let have_start = self.results.cache_start;
        let have_end = have_start + self.results.rows.len();

        let covered = first >= have_start && last < have_end;
        let room_above = have_start == 0 || first >= have_start + PREFETCH;
        let room_below = have_end >= self.results.total || last + PREFETCH < have_end;
        if covered && room_above && room_below {
            return;
        }

        // Centre the new window on the view, then pull it back inside the result
        // set so that the end of a list still gets a full window.
        let centre = (first + last) / 2;
        let end = (centre + WINDOW / 2).min(self.results.total);
        let start = end.saturating_sub(WINDOW);

        if self.pending_window == Some((start, end)) {
            return;
        }
        self.pending_window = Some((start, end));
        self.worker.send(Request::Page {
            offset: start,
            limit: end - start,
            token: self.token,
        });
    }

    fn select(&mut self, index: isize) {
        if self.results.total == 0 {
            return;
        }
        let last = self.results.total as isize - 1;
        let index = index.clamp(0, last) as usize;
        self.results.selected = Some(index);
        self.scroll_to = Some(index);
    }

    fn selected_row(&self) -> Option<&Row> {
        self.results.selected.and_then(|index| self.results.row(index))
    }

    /// Narrow the search to the folder a result sits in.
    ///
    /// Finding one file and then wanting its neighbours is the commonest
    /// follow-up there is, and retyping the path by hand is the commonest
    /// annoyance.
    fn search_in_folder(&mut self, row: &Row) {
        // A folder's own name scopes to itself; a file scopes to its parent.
        let folder = if row.is_dir { &row.path } else { &row.folder };
        let root = format!("{}:\\", self.letter);
        let relative = folder.strip_prefix(&root).unwrap_or(folder);

        self.query = if relative.is_empty() {
            String::new()
        } else {
            format!("{relative}\\")
        };
        self.focus_query = true;
        self.search_now();
    }
}

/* -------------------------------------------------------------------- *
 * The frame
 * -------------------------------------------------------------------- */

impl eframe::App for Ferret {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, prefs::KEY, &self.prefs());
    }

    /// Everything that is not drawing. Runs once before each frame, and also
    /// while the window is hidden — which is exactly when a finished scan or a
    /// journal batch has to be taken in, so the window is correct the moment it
    /// is shown again.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let events = self.worker.drain();
        let restarting = events.iter().any(|e| matches!(e, Event::Restarting));
        self.absorb(events);
        if restarting {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        self.handle_keys(ctx);

        // The debounce: a search runs once typing has settled. Asking for a
        // repaint at the deadline is what makes that happen without a timer
        // thread — egui is otherwise asleep between keystrokes.
        if let Some(since) = self.typing_since {
            let waited = since.elapsed();
            if waited >= DEBOUNCE {
                self.search_now();
            } else {
                ctx.request_repaint_after(DEBOUNCE - waited);
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.toolbar(ui);
        self.filters(ui);
        self.status_bar(ui);
        self.middle(ui);
        self.refresh_title(ui.ctx());
    }
}

impl Ferret {
    fn handle_keys(&mut self, ctx: &egui::Context) {
        let query_focused = ctx.memory(|m| m.has_focus(egui::Id::new(QUERY_ID)));
        let rows_per_screen = self.rows_per_screen;
        let current = self.results.selected.map_or(-1, |index| index as isize);

        enum Key {
            FocusSearch,
            Rescan,
            Clear,
            Move(isize),
            Home,
            End,
            Open,
            Copy,
        }
        let mut pressed = Vec::new();

        ctx.input_mut(|input| {
            use egui::{Key as K, Modifiers as M};

            if input.consume_key(M::COMMAND, K::F) {
                pressed.push(Key::FocusSearch);
            }
            if input.consume_key(M::NONE, K::F5) {
                pressed.push(Key::Rescan);
            }
            if input.consume_key(M::NONE, K::Escape) {
                pressed.push(Key::Clear);
            }
            // Arrows walk the list even while the caret is in the search box:
            // typing a few letters and then stepping down through the hits is
            // the whole interaction, and reaching for the mouse breaks it.
            if input.consume_key(M::NONE, K::ArrowDown) {
                pressed.push(Key::Move(1));
            }
            if input.consume_key(M::NONE, K::ArrowUp) {
                pressed.push(Key::Move(-1));
            }
            if input.consume_key(M::NONE, K::PageDown) {
                pressed.push(Key::Move(rows_per_screen));
            }
            if input.consume_key(M::NONE, K::PageUp) {
                pressed.push(Key::Move(-rows_per_screen));
            }
            if input.consume_key(M::NONE, K::Enter) {
                pressed.push(Key::Open);
            }
            // Home, End and Ctrl+C belong to the text while the caret is in it.
            if !query_focused {
                if input.consume_key(M::NONE, K::Home) {
                    pressed.push(Key::Home);
                }
                if input.consume_key(M::NONE, K::End) {
                    pressed.push(Key::End);
                }
                if input.consume_key(M::COMMAND, K::C) {
                    pressed.push(Key::Copy);
                }
            }
        });

        for key in pressed {
            match key {
                Key::FocusSearch => self.focus_query = true,
                Key::Rescan => {
                    if !self.scanning() && !self.letter.is_empty() {
                        self.start_scan();
                    }
                }
                Key::Clear => {
                    if !self.query.is_empty() {
                        self.query.clear();
                        self.search_now();
                    }
                }
                Key::Move(by) => self.select(current + by),
                Key::Home => self.select(0),
                Key::End => self.select(self.results.total as isize - 1),
                Key::Open => {
                    if let Some(path) = self.selected_row().map(|row| row.path.clone()) {
                        self.worker.send(Request::Open(path));
                    }
                }
                Key::Copy => {
                    if let Some(path) = self.selected_row().map(|row| row.path.clone()) {
                        ctx.copy_text(path);
                        self.inform(self.lang.strings().copied.to_string());
                    }
                }
            }
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(self.theme);
        let strings = self.lang.strings();
        let busy = self.scanning();

        egui::Panel::top("toolbar")
            .frame(
                egui::Frame::NONE
                    .fill(palette.panel)
                    .inner_margin(egui::Margin::symmetric(12, 9)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Ferret")
                            .color(palette.accent)
                            .strong()
                            .size(15.0),
                    );
                    ui.add_space(4.0);

                    // The drive, language, and the two icon buttons are laid out
                    // from the right so the search box can take the rest.
                    let mut rescan = false;
                    let mut flip_theme = false;
                    let mut changed = false;

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button(if self.theme == Theme::Dark { "☀" } else { "☾" })
                            .on_hover_text(strings.theme_tooltip)
                            .clicked()
                        {
                            flip_theme = true;
                        }

                        if ui
                            .add_enabled(!busy, egui::Button::new("⟳"))
                            .on_hover_text(strings.rescan_tooltip)
                            .clicked()
                        {
                            rescan = true;
                        }

                        let mut lang = self.lang;
                        egui::ComboBox::from_id_salt("language")
                            .width(56.0)
                            .selected_text(lang.label())
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut lang, Lang::Tr, Lang::Tr.label());
                                ui.selectable_value(&mut lang, Lang::En, Lang::En.label());
                            })
                            .response
                            .on_hover_text(strings.language_tooltip);
                        if lang != self.lang {
                            self.lang = lang;
                        }

                        let mut letter = self.letter.clone();
                        egui::ComboBox::from_id_salt("drive")
                            .width(64.0)
                            .selected_text(format!("{letter}:"))
                            .show_ui(ui, |ui| {
                                for volume in &self.volumes {
                                    ui.selectable_value(
                                        &mut letter,
                                        volume.letter.clone(),
                                        format!("{}:", volume.letter),
                                    );
                                }
                            })
                            .response
                            .on_hover_text(strings.drive_tooltip);
                        if letter != self.letter && !busy {
                            self.letter = letter;
                            changed = true;
                        }

                        // Whatever is left over goes to the search box.
                        let response = ui.add_sized(
                            egui::vec2(ui.available_width(), 26.0),
                            egui::TextEdit::singleline(&mut self.query)
                                .id(egui::Id::new(QUERY_ID))
                                .hint_text(strings.search_placeholder)
                                .margin(egui::Margin::symmetric(8, 4)),
                        );
                        let response = response.on_hover_text(strings.search_tooltip);
                        if response.changed() {
                            self.search_soon();
                        }
                        if std::mem::take(&mut self.focus_query) {
                            response.request_focus();
                        }
                    });

                    if flip_theme {
                        self.theme = self.theme.flipped();
                        theme::apply(ui.ctx(), self.theme);
                    }
                    if rescan {
                        self.start_scan();
                    }
                    if changed {
                        // A drive that is already indexed answers straight away;
                        // one that is not has to be read first.
                        let indexed = self
                            .volumes
                            .iter()
                            .find(|v| v.letter == self.letter)
                            .is_some_and(|v| v.indexed);
                        if indexed {
                            self.worker.send(Request::ListVolumes);
                            self.search_now();
                        } else {
                            self.start_scan();
                        }
                    }
                });
            });
    }

    fn filters(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(self.theme);
        let strings = self.lang.strings();

        egui::Panel::top("filters")
            .frame(
                egui::Frame::NONE
                    .fill(palette.panel)
                    .inner_margin(egui::Margin {
                        left: 12,
                        right: 12,
                        top: 0,
                        bottom: 8,
                    }),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let mut changed = false;

                    if ui.checkbox(&mut self.files_only, strings.files_only).changed() {
                        // Files-only and folders-only are mutually exclusive;
                        // ticking one clears the other rather than silently
                        // returning nothing at all.
                        if self.files_only {
                            self.dirs_only = false;
                        }
                        changed = true;
                    }
                    if ui.checkbox(&mut self.dirs_only, strings.dirs_only).changed() {
                        if self.dirs_only {
                            self.files_only = false;
                        }
                        changed = true;
                    }
                    if ui
                        .checkbox(&mut self.skip_hidden, strings.skip_hidden)
                        .changed()
                    {
                        changed = true;
                    }

                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(strings.at_least).color(palette.muted));

                    let selected = self
                        .min_size
                        .and_then(|bytes| {
                            SIZE_STEPS
                                .iter()
                                .find(|(step, _)| *step == bytes)
                                .map(|(_, label)| *label)
                        })
                        .unwrap_or(strings.any_size);

                    let mut min_size = self.min_size;
                    egui::ComboBox::from_id_salt("min-size")
                        .width(84.0)
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut min_size, None, strings.any_size);
                            for (bytes, label) in SIZE_STEPS {
                                ui.selectable_value(&mut min_size, Some(*bytes), *label);
                            }
                        });
                    if min_size != self.min_size {
                        self.min_size = min_size;
                        changed = true;
                    }

                    if let Some(notice) = &self.notice {
                        let colour = if notice.warning {
                            palette.danger
                        } else {
                            palette.muted
                        };
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new(&notice.text).color(colour).small());
                        });
                    }

                    if changed {
                        self.search_now();
                    }
                });
            });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(self.theme);

        egui::Panel::bottom("status")
            .frame(
                egui::Frame::NONE
                    .fill(palette.panel)
                    .inner_margin(egui::Margin::symmetric(12, 6)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if let Some(took) = self.results.took_ms {
                        let mut parts = vec![self.lang.results(self.results.total as u64)];
                        // The total size only says something once there is
                        // something to add up.
                        if self.results.total > 0 && !self.results.total_size.is_empty() {
                            parts.push(self.lang.total_size(&self.results.total_size));
                        }
                        parts.push(self.lang.timing(took));
                        ui.label(parts.join("  ·  "));
                    } else {
                        ui.label("—");
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(volume) = &self.volume {
                            ui.label(
                                egui::RichText::new(self.lang.volume_line(
                                    &volume.letter,
                                    self.live_files,
                                    self.live_dirs,
                                    volume.scan_seconds,
                                    &volume.memory,
                                ))
                                .color(palette.muted)
                                .small(),
                            );
                        }
                    });
                });
            });
    }

    fn middle(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(self.theme);
        // Page Up and Page Down are handled before any of this is laid out, so
        // the height is measured here and used on the next keystroke.
        self.rows_per_screen = (ui.available_height() / ROW_HEIGHT).floor().max(1.0) as isize;

        egui::CentralPanel::no_frame()
            .frame(egui::Frame::NONE.fill(palette.bg))
            .show(ui, |ui| match &self.phase {
                Phase::Starting => {
                    waiting(ui, self.lang.strings().preparing, "", None);
                }
                Phase::Scanning { letter, percent } => {
                    let title = self.lang.scanning(&letter.to_string());
                    let detail = self.lang.strings().scanning_detail;
                    waiting(ui, &title, detail, Some(*percent));
                }
                Phase::NoVolume => {
                    let strings = self.lang.strings();
                    card(ui, strings.no_volume_title, strings.no_volume_detail, None);
                }
                Phase::NeedsElevation => {
                    let strings = self.lang.strings();
                    let clicked = card(
                        ui,
                        strings.elevation_title,
                        strings.elevation_detail,
                        Some(strings.elevation_action),
                    );
                    if clicked {
                        self.worker.send(Request::RestartElevated);
                    }
                }
                Phase::ScanFailed { letter, message } => {
                    let letter = *letter;
                    let detail = self.lang.error(message);
                    let strings = self.lang.strings();
                    let clicked = card(ui, strings.scan_failed, &detail, Some(strings.retry));
                    if clicked {
                        self.letter = letter.to_string();
                        self.start_scan();
                    }
                }
                Phase::Ready => self.table(ui),
            });
    }

    fn table(&mut self, ui: &mut egui::Ui) {
        if self.results.total == 0 && self.results.took_ms.is_some() {
            let title = self.lang.empty_title(self.query.trim());
            card(ui, &title, self.lang.strings().empty_hint, None);
            return;
        }

        let outcome = draw_table(
            ui,
            self.lang,
            self.theme,
            &self.results,
            &self.sort_by,
            self.descending,
            self.scroll_to.take(),
        );

        if let Some((first, last)) = outcome.visible {
            self.ensure_window(first, last);
        }
        if let Some(index) = outcome.select {
            self.results.selected = Some(index);
        }
        if let Some(column) = outcome.sort {
            if self.sort_by == column {
                self.descending = !self.descending;
            } else {
                self.sort_by = column.to_string();
                self.descending = false;
            }
            self.search_now();
        }
        if let Some(path) = outcome.open {
            self.worker.send(Request::Open(path));
        }
        if let Some(path) = outcome.reveal {
            self.worker.send(Request::Reveal(path));
        }
        if let Some(path) = outcome.copy {
            ui.ctx().copy_text(path);
            self.inform(self.lang.strings().copied.to_string());
        }
        if let Some(row) = outcome.search_here {
            self.search_in_folder(&row);
        }
    }

    fn refresh_title(&mut self, ctx: &egui::Context) {
        let wanted = match (&self.volume, self.query.trim().is_empty()) {
            (_, false) => self.lang.title_results(self.results.total as u64),
            (Some(volume), true) => self.lang.title_volume(&volume.letter, self.live_files),
            (None, true) => "Ferret".to_string(),
        };
        if wanted != self.title {
            self.title = wanted.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(wanted));
        }
    }
}

/* -------------------------------------------------------------------- *
 * The result table
 * -------------------------------------------------------------------- */

/// What the table would like the app to do. Collected rather than done on the
/// spot, because the table borrows the results it is drawing and cannot also
/// hand out the mutable borrow that acting on a click would need.
#[derive(Default)]
struct Outcome {
    select: Option<usize>,
    open: Option<String>,
    reveal: Option<String>,
    copy: Option<String>,
    search_here: Option<Row>,
    sort: Option<&'static str>,
    /// The first and last row index drawn this frame.
    visible: Option<(usize, usize)>,
}

#[allow(clippy::too_many_arguments)]
fn draw_table(
    ui: &mut egui::Ui,
    lang: Lang,
    theme: Theme,
    results: &Results,
    sort_by: &str,
    descending: bool,
    scroll_to: Option<usize>,
) -> Outcome {
    let palette = theme::palette(theme);
    let strings = lang.strings();
    let mut outcome = Outcome::default();

    let mut builder = TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .sense(egui::Sense::click())
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::initial(300.0).at_least(140.0).clip(true))
        .column(Column::remainder().at_least(120.0).clip(true))
        .column(Column::initial(66.0).at_least(44.0).clip(true))
        .column(Column::initial(92.0).at_least(60.0).clip(true))
        .column(Column::initial(132.0).at_least(90.0).clip(true))
        .min_scrolled_height(0.0);

    if let Some(row) = scroll_to {
        builder = builder.scroll_to_row(row, Some(egui::Align::Center));
    }

    builder
        .header(26.0, |mut header| {
            for (column, label) in [
                (Some("name"), strings.col_name),
                (Some("path"), strings.col_path),
                (None, strings.col_kind),
                (Some("size"), strings.col_size),
                (Some("modified"), strings.col_date),
            ] {
                header.col(|ui| {
                    let arrow = match (column, column == Some(sort_by), descending) {
                        (Some(_), true, false) => "  ↑",
                        (Some(_), true, true) => "  ↓",
                        _ => "",
                    };
                    let text = egui::RichText::new(format!("{label}{arrow}"))
                        .color(if column == Some(sort_by) {
                            palette.accent
                        } else {
                            palette.muted
                        })
                        .small();

                    match column {
                        Some(column) => {
                            if ui
                                .add(egui::Button::new(text).frame(false))
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                            {
                                outcome.sort = Some(column);
                            }
                        }
                        // There is nothing to sort a type column by that the
                        // name column does not already do better.
                        None => {
                            ui.label(text);
                        }
                    }
                });
            }
        })
        .body(|body| {
            body.rows(ROW_HEIGHT, results.total, |mut table_row| {
                let index = table_row.index();
                outcome.visible = Some(match outcome.visible {
                    Some((first, last)) => (first.min(index), last.max(index)),
                    None => (index, index),
                });
                table_row.set_selected(results.selected == Some(index));

                match results.row(index) {
                    Some(row) => {
                        table_row.col(|ui| {
                            icon(ui, row.is_dir, palette);
                            ui.add(
                                egui::Label::new(highlighted(
                                    ui,
                                    &row.name,
                                    &results.needle,
                                    palette,
                                ))
                                .selectable(false)
                                .truncate(),
                            );
                        });
                        table_row.col(|ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&row.folder).color(palette.muted),
                                )
                                .selectable(false)
                                .truncate(),
                            );
                        });
                        table_row.col(|ui| {
                            let kind = if row.is_dir { strings.folder } else { &row.kind };
                            ui.add(
                                egui::Label::new(egui::RichText::new(kind).color(palette.muted))
                                    .selectable(false)
                                    .truncate(),
                            );
                        });
                        table_row.col(|ui| {
                            // Right-aligned, because a column of sizes is read
                            // by comparing magnitudes, not by reading words.
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.add(
                                        egui::Label::new(&row.size)
                                            .selectable(false)
                                            .truncate(),
                                    );
                                },
                            );
                        });
                        table_row.col(|ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&row.modified).color(palette.muted),
                                )
                                .selectable(false)
                                .truncate(),
                            );
                        });
                    }
                    // The row is on screen but its window has not arrived. One
                    // frame of a faint placeholder beats a blank table.
                    None => {
                        for _ in 0..5 {
                            table_row.col(|ui| {
                                ui.label(egui::RichText::new("…").color(palette.muted));
                            });
                        }
                    }
                }

                let response = table_row.response();
                if response.clicked() {
                    outcome.select = Some(index);
                }
                if response.double_clicked() {
                    if let Some(row) = results.row(index) {
                        outcome.open = Some(row.path.clone());
                    }
                }
                if let Some(row) = results.row(index) {
                    response.context_menu(|ui| {
                        outcome.select = Some(index);
                        ui.set_min_width(180.0);
                        if ui.button(strings.menu_open).clicked() {
                            outcome.open = Some(row.path.clone());
                            ui.close();
                        }
                        if ui.button(strings.menu_reveal).clicked() {
                            outcome.reveal = Some(row.path.clone());
                            ui.close();
                        }
                        if ui.button(strings.menu_copy).clicked() {
                            outcome.copy = Some(row.path.clone());
                            ui.close();
                        }
                        if ui.button(strings.menu_search_here).clicked() {
                            outcome.search_here = Some(row.clone());
                            ui.close();
                        }
                    });
                }
            });
        });

    outcome
}

/// A folder or a file, drawn rather than written.
///
/// Two shapes are all a file list needs, and painting them costs nothing next to
/// loading an icon font for the same result.
fn icon(ui: &mut egui::Ui, is_dir: bool, palette: &theme::Palette) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
    let painter = ui.painter();
    let colour = if is_dir { palette.accent } else { palette.muted };

    if is_dir {
        let body = egui::Rect::from_min_max(
            rect.left_top() + egui::vec2(1.0, 3.5),
            rect.right_bottom() - egui::vec2(1.0, 1.5),
        );
        let tab = egui::Rect::from_min_size(
            rect.left_top() + egui::vec2(1.0, 1.5),
            egui::vec2(6.0, 2.5),
        );
        painter.rect_filled(tab, 1.0, colour);
        painter.rect_filled(body, 2.0, colour);
    } else {
        let body = egui::Rect::from_min_max(
            rect.left_top() + egui::vec2(2.5, 1.0),
            rect.right_bottom() - egui::vec2(2.5, 1.0),
        );
        painter.rect_stroke(
            body,
            1.5,
            egui::Stroke::new(1.2, colour),
            egui::StrokeKind::Inside,
        );
    }
    ui.add_space(2.0);
}

/// A file name with the matched part standing out.
///
/// Highlighting is what turns a list of similar names into a scannable one: the
/// eye finds the accented run without reading anything.
fn highlighted(
    ui: &egui::Ui,
    name: &str,
    needle: &str,
    palette: &theme::Palette,
) -> egui::text::LayoutJob {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let mut job = egui::text::LayoutJob::default();

    let plain = egui::TextFormat {
        font_id: font.clone(),
        color: palette.text,
        ..Default::default()
    };
    let marked = egui::TextFormat {
        font_id: font,
        color: palette.accent,
        background: palette.accent_soft,
        ..Default::default()
    };

    match find_ignoring_case(name, needle) {
        Some((start, end)) => {
            job.append(&name[..start], 0.0, plain.clone());
            job.append(&name[start..end], 0.0, marked);
            job.append(&name[end..], 0.0, plain);
        }
        None => job.append(name, 0.0, plain),
    }
    job
}

/// Where `needle` sits inside `haystack`, ignoring case, as a byte range.
///
/// ASCII — which is nearly every query — is compared byte by byte and allocates
/// nothing. Anything else is folded properly, because Turkish is exactly the
/// language where byte-wise lowercasing goes wrong: `İ` is two bytes and folds
/// to two characters, so the naive version reports a range that does not line up
/// with the original string at all.
fn find_ignoring_case(haystack: &str, needle: &str) -> Option<(usize, usize)> {
    if needle.is_empty() || haystack.is_empty() {
        return None;
    }

    if haystack.is_ascii() && needle.is_ascii() {
        let hay = haystack.as_bytes();
        let pin = needle.as_bytes();
        if pin.len() > hay.len() {
            return None;
        }
        return (0..=hay.len() - pin.len())
            .find(|start| {
                hay[*start..*start + pin.len()]
                    .iter()
                    .zip(pin)
                    .all(|(a, b)| a.eq_ignore_ascii_case(b))
            })
            .map(|start| (start, start + pin.len()));
    }

    // Each folded character remembers the byte it came from, so a match can be
    // reported as a range in the original string.
    let folded: Vec<(usize, char)> = haystack
        .char_indices()
        .flat_map(|(at, ch)| ch.to_lowercase().map(move |lower| (at, lower)))
        .collect();
    let pin: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    if pin.is_empty() || pin.len() > folded.len() {
        return None;
    }

    (0..=folded.len() - pin.len())
        .find(|start| {
            folded[*start..*start + pin.len()]
                .iter()
                .zip(&pin)
                .all(|((_, a), b)| a == b)
        })
        .map(|start| {
            let from = folded[start].0;
            let to = folded
                .get(start + pin.len())
                .map(|(at, _)| *at)
                .unwrap_or(haystack.len());
            (from, to)
        })
}

/* -------------------------------------------------------------------- *
 * The screens that are not the table
 * -------------------------------------------------------------------- */

/// A scan in progress, with a bar rather than a spinner.
///
/// Seven seconds behind something that says nothing is the app's worst moment;
/// a bar that visibly moves turns the same wait into something ordinary.
fn waiting(ui: &mut egui::Ui, title: &str, detail: &str, percent: Option<u8>) {
    centred(ui, |ui| {
        ui.add(egui::Spinner::new().size(28.0));
        ui.add_space(14.0);
        ui.label(egui::RichText::new(title).heading());
        if !detail.is_empty() {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(detail).weak());
        }
        if let Some(percent) = percent {
            ui.add_space(14.0);
            ui.add_sized(
                egui::vec2(280.0, 8.0),
                egui::ProgressBar::new(percent as f32 / 100.0).corner_radius(4),
            );
        }
    });
}

/// A message with an optional button. Returns whether the button was pressed.
fn card(ui: &mut egui::Ui, title: &str, detail: &str, action: Option<&str>) -> bool {
    let mut clicked = false;
    centred(ui, |ui| {
        ui.label(egui::RichText::new(title).heading());
        if !detail.is_empty() {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(detail).weak());
        }
        if let Some(action) = action {
            ui.add_space(16.0);
            clicked = ui.button(action).clicked();
        }
    });
    clicked
}

/// Put a small block of content in the middle of the available space.
fn centred(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui)) {
    let height = ui.available_height();
    ui.vertical_centered(|ui| {
        ui.add_space(height * 0.28);
        ui.set_max_width(440.0);
        contents(ui);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, is_dir: bool) -> Row {
        Row {
            name: name.to_string(),
            path: format!(r"C:\x\{name}"),
            folder: r"C:\x".to_string(),
            size: "1.0 KB".into(),
            size_bytes: 1024,
            modified: "2026-01-01 00:00".into(),
            kind: "TXT".into(),
            is_dir,
        }
    }

    #[test]
    fn the_cache_answers_only_for_the_window_it_holds() {
        let results = Results {
            total: 5_000,
            cache_start: 1_000,
            rows: vec![row("a.txt", false), row("b.txt", false)],
            ..Default::default()
        };

        assert_eq!(results.row(1_000).map(|r| r.name.as_str()), Some("a.txt"));
        assert_eq!(results.row(1_001).map(|r| r.name.as_str()), Some("b.txt"));
        // Outside the window there is nothing yet, and no panic either.
        assert!(results.row(999).is_none());
        assert!(results.row(1_002).is_none());
        assert!(results.row(0).is_none());
        assert!(results.row(4_999).is_none());
    }

    #[test]
    fn a_match_is_found_whatever_case_it_was_typed_in() {
        assert_eq!(find_ignoring_case("Rapor.pdf", "rapor"), Some((0, 5)));
        assert_eq!(find_ignoring_case("rapor.pdf", "RAPOR"), Some((0, 5)));
        assert_eq!(find_ignoring_case("yillik-rapor.pdf", "rapor"), Some((7, 12)));
        assert_eq!(find_ignoring_case("rapor.pdf", "PDF"), Some((6, 9)));
    }

    #[test]
    fn nothing_to_highlight_is_not_a_match() {
        assert_eq!(find_ignoring_case("rapor.pdf", ""), None);
        assert_eq!(find_ignoring_case("", "rapor"), None);
        assert_eq!(find_ignoring_case("a", "abc"), None);
        assert_eq!(find_ignoring_case("rapor.pdf", "xyz"), None);
    }

    /// The reason the non-ASCII path exists: a byte-wise lowercase would report
    /// a range that does not line up with the original string.
    #[test]
    fn a_turkish_name_highlights_the_right_bytes() {
        let name = "YILLIK Özet.docx";
        let (start, end) = find_ignoring_case(name, "özet").expect("the word is there");

        // The range has to be usable to slice the original, not the folded copy.
        assert_eq!(&name[start..end], "Özet");
        assert!(name.is_char_boundary(start) && name.is_char_boundary(end));
    }

    #[test]
    fn a_match_at_the_very_end_of_a_non_ascii_name_stays_in_bounds() {
        let name = "belge-şubat";
        let (start, end) = find_ignoring_case(name, "ŞUBAT").expect("the word is there");
        assert_eq!(&name[start..end], "şubat");
        assert_eq!(end, name.len());
    }

    /// Whatever the two paths do, they must agree.
    #[test]
    fn both_paths_report_the_same_range_for_ascii() {
        // A needle that forces the folding path. The ß is the fifth character
        // but the fifth *byte* onwards, because the four before it are ASCII.
        let name = "straße.txt";
        assert_eq!(find_ignoring_case(name, "ß"), Some((4, 6)));
        assert_eq!(&name[4..6], "ß");
        // And an accented haystack with an ASCII needle.
        let name = "Ödev.txt";
        let (start, end) = find_ignoring_case(name, "txt").expect("the extension is there");
        assert_eq!(&name[start..end], "txt");
    }
}
