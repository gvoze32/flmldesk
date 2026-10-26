//! egui front-end: transfers and budget for an ongoing Master League save.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui::{self, RichText, vec2};
use egui_extras::{Column, TableBuilder};
use fl_ml_desk::masterleague::{MasterLeague, Player, Source, TransferRequest, fmt_int};
use fl_ml_desk::contracts::Date;
use fl_ml_desk::players::{PlayerCatalog, PlayerInfo};
use fl_ml_desk::{Error, update};
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

use crate::theme::{self, Tone};

const BUNDLED_NAMES: &str = include_str!("../data/FL2622wc_players.txt");
const SETTINGS_FILE: &str = ".fl-ml-desk.json";
/// Display currencies use the game's own conversion factors (EUR is the stored base).
const CURRENCIES: [(&str, f64); 5] = [("EUR €", 1.0), ("GBP £", 0.9), ("CHF", 1.1), ("BRL R$", 4.3), ("JPY ¥", 125.0)];

fn normal(text: &str) -> String {
    text.nfkd().filter(|c| !is_combining_mark(*c)).collect::<String>().to_lowercase()
}

/// `12,500,000`, `12.5m`, `750k` -> integer (ties to even, like the game tools).
fn parse_amount(text: &str) -> Result<i64, ()> {
    let mut s: String = text.trim().to_lowercase().chars().filter(|c| *c != ' ').collect();
    let mut mult = 1.0;
    for (suffix, factor) in [("m", 1e6), ("k", 1e3)] {
        if let Some(rest) = s.strip_suffix(suffix) {
            s = rest.to_string();
            mult = factor;
            break;
        }
    }
    if s.is_empty() || !s.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ',') {
        return Err(());
    }
    // Commas group thousands; a dot is the decimal point.
    let value: f64 = s.replace(',', "").parse().map_err(|_| ())?;
    if !value.is_finite() || value < 0.0 {
        return Err(());
    }
    Ok((value * mult).round_ties_even() as i64)
}

fn settings_path() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(|h| PathBuf::from(h).join(SETTINGS_FILE))
}

#[derive(Serialize, Deserialize, Default)]
struct Settings {
    #[serde(default)]
    game_folder: Option<String>,
    #[serde(default)]
    currency: Option<String>,
    /// Managed club per save path.
    #[serde(default)]
    clubs: HashMap<String, usize>,
}

impl Settings {
    fn load() -> Self {
        settings_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn store(&self) {
        if let (Some(path), Ok(text)) = (settings_path(), serde_json::to_string_pretty(self)) {
            let _ = std::fs::write(path, text);
        }
    }
}

enum Job {
    Open(Result<(Box<MasterLeague>, Option<(PlayerCatalog, usize)>), Error>),
    Names(Result<(PlayerCatalog, usize, String), Error>),
    Update { manual: bool, result: Result<Option<update::Release>, Error> },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Transfer,
    Budget,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceFilter {
    All,
    Mine,
    Club(usize),
    Others,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    In,
    Out,
    Ai,
}

struct ResultRow {
    source: Source,
    player: Player,
    info: PlayerInfo,
    club: String,
    wage: String,
}

fn error_dialog(title: &str, message: &str) {
    MessageDialog::new().set_level(MessageLevel::Error).set_title(title).set_description(message).show();
}

fn confirm(title: &str, message: &str) -> bool {
    MessageDialog::new()
        .set_level(MessageLevel::Warning)
        .set_title(title)
        .set_description(message)
        .set_buttons(MessageButtons::YesNo)
        .show()
        == MessageDialogResult::Yes
}

/// Combobox with a filter box; returns the picked value when the selection changes.
fn search_combo<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    id: &str,
    width: f32,
    selected_text: &str,
    filter: &mut String,
    items: &[(String, T)],
) -> Option<T> {
    let mut picked = None;
    egui::ComboBox::from_id_salt(id)
        .width(width)
        .selected_text(selected_text)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            ui.add(egui::TextEdit::singleline(filter).hint_text("Search…").desired_width(width - 20.0)).request_focus();
            let q = normal(filter);
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for (label, value) in items.iter().filter(|(l, _)| q.is_empty() || normal(l).contains(&q)) {
                    if ui.selectable_label(label == selected_text, label).clicked() {
                        picked = Some(*value);
                        ui.close();
                    }
                }
            });
        });
    if picked.is_some() {
        filter.clear();
    }
    picked
}

pub struct App {
    game: Option<MasterLeague>,
    catalog: PlayerCatalog,
    settings: Settings,
    tx: Sender<Job>,
    rx: Receiver<Job>,
    busy: usize,
    status: String,
    tab: Tab,
    currency: usize,
    // club pickers
    club_items: Vec<(String, usize)>,
    my_club_filter: String,
    club_info: String,
    // transfer tab
    query: String,
    source: SourceFilter,
    source_filter: String,
    results: Vec<ResultRow>,
    results_dirty: bool,
    selected: Option<(Source, Player)>,
    dest: Option<usize>,
    dest_filter: String,
    fee: String,
    salary: String,
    end_year: u16,
    end_month: u8,
    end_day: u8,
    use_tb: bool,
    // budget tab
    tb: String,
    sb: String,
    manager: String,
    // updates
    update: Option<update::Release>,
    logo: egui::TextureHandle,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let (tx, rx) = channel();
        let settings = Settings::load();
        let currency = settings
            .currency
            .as_deref()
            .and_then(|c| CURRENCIES.iter().position(|(n, _)| *n == c))
            .unwrap_or(0);
        let mut catalog = PlayerCatalog::default();
        catalog.load_name_list_text(BUNDLED_NAMES, "FL2622wc_players.txt");
        let mut app = App {
            game: None,
            catalog,
            settings,
            tx,
            rx,
            busy: 0,
            status: "Open a Master League save (ML000000xx). In the game, save first and go back to the main menu.".into(),
            tab: Tab::Transfer,
            currency,
            club_items: Vec::new(),
            my_club_filter: String::new(),
            club_info: String::new(),
            query: String::new(),
            source: SourceFilter::All,
            source_filter: String::new(),
            results: Vec::new(),
            results_dirty: true,
            selected: None,
            dest: None,
            dest_filter: String::new(),
            fee: "0".into(),
            salary: String::new(),
            end_year: 2029,
            end_month: 6,
            end_day: 30,
            use_tb: true,
            tb: String::new(),
            sb: String::new(),
            manager: String::new(),
            update: None,
            logo: logo_texture(&cc.egui_ctx),
        };
        if let Some(folder) = app.settings.game_folder.clone().filter(|f| Path::new(f).is_dir()) {
            app.load_game_folder(&cc.egui_ctx, PathBuf::from(folder));
        }
        app.check_updates(&cc.egui_ctx, false);
        if let Some(path) = std::env::args_os().nth(1).map(PathBuf::from).filter(|p| p.is_file()) {
            app.open_path(&cc.egui_ctx, path);
        }
        app
    }

    // --- background jobs ----------------------------------------------------
    fn spawn(&mut self, ctx: &egui::Context, work: impl FnOnce() -> Job + Send + 'static) {
        self.busy += 1;
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(work());
            ctx.request_repaint();
        });
    }

    fn poll_jobs(&mut self) {
        while let Ok(job) = self.rx.try_recv() {
            match job {
                Job::Open(result) => {
                    self.busy -= 1;
                    match result {
                        Ok((game, edit)) => self.opened(*game, edit),
                        Err(e) => self.fail("Couldn't open the save", &e),
                    }
                }
                Job::Names(result) => {
                    self.busy -= 1;
                    match result {
                        Ok((catalog, count, source)) => {
                            self.catalog.merge(catalog);
                            self.status = format!("{} players loaded from {source}.", fmt_int(count as i64));
                            self.results_dirty = true;
                        }
                        Err(e) => self.fail("Couldn't load player names", &e),
                    }
                }
                Job::Update { manual, result } => {
                    self.busy -= 1;
                    match result {
                        Ok(Some(release)) => {
                            self.status = format!("New version {} available.", release.tag);
                            self.update = Some(release);
                        }
                        Ok(None) if manual => {
                            self.status = format!("You have the latest version (v{}).", update::CURRENT_VERSION);
                        }
                        Err(e) if manual => self.fail("Check for updates", &e),
                        _ => {}
                    }
                }
            }
        }
    }

    fn fail(&mut self, title: &str, e: &Error) {
        self.status = format!("Error: {e}");
        error_dialog(title, &e.to_string());
    }

    fn check_updates(&mut self, ctx: &egui::Context, manual: bool) {
        self.spawn(ctx, move || Job::Update { manual, result: update::check() });
    }

    fn load_game_folder(&mut self, ctx: &egui::Context, folder: PathBuf) {
        self.spawn(ctx, move || {
            let mut catalog = PlayerCatalog::default();
            Job::Names(catalog.load_game_folder(&folder).map(|n| (catalog, n, format!("game folder {}", folder.display()))))
        });
    }

    // --- money helpers --------------------------------------------------------
    fn rate(&self) -> f64 {
        CURRENCIES[self.currency].1
    }

    fn to_display(&self, eur: i64) -> String {
        fmt_int((eur as f64 * self.rate()).round_ties_even() as i64)
    }

    fn from_display(&self, text: &str) -> Result<i64, ()> {
        Ok((parse_amount(text)? as f64 / self.rate()).round_ties_even() as i64)
    }

    // --- save file ------------------------------------------------------------
    fn open_save(&mut self, ctx: &egui::Context) {
        if let Some(path) = FileDialog::new().set_title("Choose a Master League save").pick_file() {
            self.open_path(ctx, path);
        }
    }

    fn open_path(&mut self, ctx: &egui::Context, path: PathBuf) {
        if self.game.as_ref().is_some_and(|g| !g.history.is_empty())
            && !confirm("Unsaved changes", "Discard your changes and open another save?")
        {
            return;
        }
        self.spawn(ctx, move || {
            Job::Open(MasterLeague::open(&path).map(|game| {
                let edit = path.parent().map(|p| p.join("EDIT00000000")).filter(|p| p.exists()).and_then(|p| {
                    let mut catalog = PlayerCatalog::default();
                    catalog.load_edit(&p).ok().map(|n| (catalog, n))
                });
                (Box::new(game), edit)
            }))
        });
    }

    fn opened(&mut self, game: MasterLeague, edit: Option<(PlayerCatalog, usize)>) {
        let mut msg = String::from("Save opened.");
        if let Some((catalog, n)) = edit {
            self.catalog.merge(catalog);
            msg += &format!(" Names loaded from EDIT00000000 ({} players).", fmt_int(n as i64));
        }
        if let Some(e) = &game.finance_error {
            msg += &format!(" WARNING: {e}");
        }
        let mut items: Vec<(String, usize)> =
            game.teams.iter().map(|t| (format!("{} ({}) #{}", t.name, t.abbr, t.index), t.index)).collect();
        items.sort_by_cached_key(|(l, _)| normal(l));
        self.club_items = items;
        let remembered = self.settings.clubs.get(&game.path.to_string_lossy().into_owned()).copied();
        let described = game.described_club;
        if let Some(date) = game.game_date {
            msg += &format!(" In-game date: {date}.");
        }
        self.game = Some(game);
        self.club_info = "First choose the club you manage (needed to buy or sell players).".into();
        self.source = SourceFilter::All;
        self.selected = None;
        self.dest = None;
        self.fill_finances();
        self.results_dirty = true;
        self.status = msg;
        if let Some(index) = described.or(remembered).filter(|i| *i < self.club_items.len()) {
            self.set_managed_club(index);
        }
    }

    fn club_label(&self, index: usize) -> String {
        let t = &self.game.as_ref().unwrap().teams[index];
        format!("{} ({}) #{}", t.name, t.abbr, t.index)
    }

    fn set_managed_club(&mut self, index: usize) {
        let Some(game) = self.game.as_mut() else { return };
        if game.managed_team == Some(index) {
            return;
        }
        match game.set_managed_team(index) {
            Ok(_) => {
                self.settings.clubs.insert(game.path.to_string_lossy().into_owned(), index);
                self.settings.store();
                let end = game.contract_end_in(3);
                (self.end_year, self.end_month, self.end_day) = (end.year, end.month, end.day);
                self.dest = Some(index);
                self.club_info.clear();
                self.results_dirty = true;
                // The game shows the salary budget minus this club's salaries.
                self.fill_finances();
            }
            Err(e) => error_dialog("Can't use this club", &e.to_string()),
        }
    }

    fn save(&mut self, target: Option<PathBuf>) {
        let Some(game) = self.game.as_mut() else { return };
        match game.write(target.as_deref()) {
            Ok((target, backup)) => {
                let note = backup.as_ref().map(|b| format!("\nBackup of the old file: {}", b.display())).unwrap_or_default();
                self.status = format!("Saved: {}.", target.display());
                MessageDialog::new()
                    .set_title("Saved")
                    .set_description(format!(
                        "Save written and checked:\n{}{note}\n\nLoad this save in the game and check your squad and finances.",
                        target.display()
                    ))
                    .show();
                self.after_change();
            }
            Err(e) => self.fail("Couldn't save", &e),
        }
    }

    fn after_change(&mut self) {
        self.fill_finances();
        self.results_dirty = true;
    }

    /// Fills the budget fields with the values the game shows. The game shows
    /// the stored yearly salary budget minus the club's salaries.
    fn fill_finances(&mut self) {
        let Some(game) = &self.game else { return };
        match game.finances() {
            Ok(f) => {
                self.tb = self.to_display(f.transfer_eur);
                self.sb = self.to_display(f.salary_eur - game.wage_bill_eur());
                self.manager = f.manager_name;
            }
            Err(_) => {
                for s in [&mut self.tb, &mut self.sb, &mut self.manager] {
                    s.clear();
                }
            }
        }
    }

    // --- transfer logic ---------------------------------------------------------
    fn refresh_results(&mut self) {
        self.results_dirty = false;
        self.results.clear();
        let Some(game) = &self.game else { return };
        let mine = game.managed_team;
        let sources: Vec<Source> = match self.source {
            SourceFilter::All => (0..game.teams.len()).map(Source::Club).chain([Source::Others]).collect(),
            SourceFilter::Mine => mine.map(Source::Club).into_iter().collect(),
            SourceFilter::Club(i) => vec![Source::Club(i)],
            SourceFilter::Others => vec![Source::Others],
        };
        let q = normal(self.query.trim());
        let mut rows = Vec::new();
        for source in sources {
            for player in game.roster(source) {
                let info = self.catalog.get(player.player_id);
                if !q.is_empty() && !normal(&format!("{} {}", info.name, player.player_id)).contains(&q) {
                    continue;
                }
                let club = match source {
                    Source::Club(i) => game.teams[i].name.clone(),
                    Source::Others => "Others (national team, check in game)".into(),
                };
                let wage = if Some(source) == mine.map(Source::Club) {
                    game.contract_of(&player).map_or("?".into(), |c| self.to_display(c.annual_eur() as i64))
                } else {
                    String::new()
                };
                rows.push(ResultRow { source, player, info, club, wage });
            }
        }
        rows.sort_by_cached_key(|r| normal(&r.info.name));
        if let Some((s, p)) = self.selected
            && !rows.iter().any(|r| r.source == s && r.player == p)
        {
            self.selected = None;
        }
        self.results = rows;
    }

    fn select(&mut self, source: Source, player: Player) {
        self.selected = Some((source, player));
        let Some(mine) = self.game.as_ref().and_then(|g| g.managed_team) else { return };
        if source == Source::Club(mine) && self.dest == Some(mine) {
            self.dest = self.club_items.iter().map(|(_, i)| *i).find(|i| *i != mine);
        } else if source != Source::Club(mine) {
            self.dest = Some(mine);
        }
    }

    fn mode(&self) -> Option<Mode> {
        let (source, _) = self.selected?;
        let dest = self.dest?;
        let mine = self.game.as_ref()?.managed_team;
        Some(if Some(dest) == mine {
            Mode::In
        } else if Some(source) == mine.map(Source::Club) {
            Mode::Out
        } else {
            Mode::Ai
        })
    }

    fn preview(&self, mode: Option<Mode>) -> Vec<String> {
        let Some(game) = &self.game else { return vec![] };
        let Some(mode) = mode else {
            return if game.managed_team.is_none() {
                vec!["Choose your club first, so buying and selling can update contracts and budget.".into()]
            } else {
                vec![]
            };
        };
        if mode == Mode::Ai {
            return vec!["Transfer between two AI clubs: only the squads change. Their fees and contracts stay the same.".into()];
        }
        let Ok(fee) = self.from_display(if self.fee.trim().is_empty() { "0" } else { &self.fee }) else {
            return vec!["The fee isn't a valid amount.".into()];
        };
        let fin = game.finances().ok();
        let wages = game.wage_bill_eur();
        let mut lines = Vec::new();
        if mode == Mode::In {
            if let (true, Some(f)) = (self.use_tb, &fin) {
                lines.push(format!("Transfer budget: {} → {}", self.to_display(f.transfer_eur), self.to_display(f.transfer_eur - fee)));
            }
            if let Some(f) = &fin {
                let left = f.salary_eur - wages;
                lines.push(match self.from_display(&self.salary) {
                    Ok(wage) => format!("Salary budget left (in game): {} → {}", self.to_display(left), self.to_display(left - wage)),
                    Err(()) => format!("Salary budget left (in game): {}", self.to_display(left)),
                });
            }
            if let Some(day) = game.game_date.map(|d| d.next_day()) {
                lines.push(format!(
                    "Deal agreed now: the player joins on {day}, when the game moves him. Until then he shows in your negotiations."
                ));
            }
            if matches!(self.selected, Some((Source::Others, _))) && fee != 0 {
                lines.push("A player without a club only negotiates his salary in the game; leave the fee at 0.".into());
            }
        } else {
            let contract = self.selected.and_then(|(_, p)| game.contract_of(&p));
            match contract {
                Some(c) => lines.push(format!("Contract of {}/yr until {} will be removed.", self.to_display(c.annual_eur() as i64), c.end)),
                None => lines.push("This player's contract couldn't be found, so the transfer will be cancelled.".into()),
            }
            if let (true, Some(f)) = (self.use_tb, &fin) {
                lines.push(format!("Transfer budget: {} → {}", self.to_display(f.transfer_eur), self.to_display(f.transfer_eur + fee)));
            }
        }
        if game.finance_error.is_some() && self.use_tb {
            lines.push("The budget couldn't be read from this save. Untick \"Pay fee from transfer budget\" to continue.".into());
        }
        lines
    }

    fn do_transfer(&mut self) {
        let (Some((source, player)), Some(dest), Some(mode)) = (self.selected, self.dest, self.mode()) else {
            error_dialog("Missing details", "Choose a player and a destination club.");
            return;
        };
        let info = self.catalog.get(player.player_id);
        let mut req = TransferRequest {
            use_transfer_budget: self.use_tb,
            name: Some(info.name.clone()),
            ..TransferRequest::new(source, dest, player.player_id)
        };
        let parsed = (|| -> Result<(), ()> {
            if mode != Mode::Ai {
                req.fee_eur = self.from_display(if self.fee.trim().is_empty() { "0" } else { &self.fee })?;
            }
            if mode == Mode::In {
                req.salary_eur = Some(self.from_display(&self.salary)?);
                req.contract_end = Some(Date::new(self.end_year, self.end_month, self.end_day).ok_or(())?);
            }
            Ok(())
        })();
        if parsed.is_err() {
            error_dialog("Invalid details", "Check the fee, salary and contract date.");
            return;
        }
        let game = self.game.as_mut().unwrap();
        match game.transfer(req) {
            Ok(op) => {
                self.status = format!("Added: {}  — press Save to write it to the file.", op.description);
                self.after_change();
            }
            Err(e) => error_dialog("Transfer cancelled", &e.to_string()),
        }
    }

    fn apply_finances(&mut self) {
        let parsed = (|| -> Result<(i64, i64), ()> { Ok((self.from_display(&self.tb)?, self.from_display(&self.sb)?)) })();
        let Ok((tb, sb_left)) = parsed else {
            error_dialog("Invalid amount", "Check the budget amounts again.");
            return;
        };
        let manager = self.manager.clone();
        let game = self.game.as_mut().unwrap();
        // The field holds what the game shows; the save stores it plus the salaries.
        let sb = sb_left + game.wage_bill_eur();
        match game.set_finances(tb, sb, &manager) {
            Ok(op) => {
                self.status = op.description.clone();
                self.after_change();
            }
            Err(e) => error_dialog("Couldn't do that", &e.to_string()),
        }
    }

    // --- layout ---------------------------------------------------------------------
    fn header(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.add(egui::Image::from_texture(egui::load::SizedTexture::from_handle(&self.logo)).fit_to_exact_size(vec2(44.0, 44.0)));
            ui.vertical(|ui| {
                ui.add_space(2.0);
                ui.label(RichText::new("FL ML Desk").heading().strong().color(theme::accent(ui)));
                ui.label(RichText::new("Master League transfers & budget · Football Life 2026 / PES 2021").color(theme::muted(ui)));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Check for updates").clicked() {
                    self.check_updates(&ctx, true);
                }
                ui.label(RichText::new(format!("v{}", update::CURRENT_VERSION)).small().color(theme::muted(ui)));
                if self.busy > 0 {
                    ui.spinner();
                }
            });
        });
        if let Some(release) = self.update.clone() {
            ui.add_space(4.0);
            theme::callout(ui, Tone::Info, |ui| {
                ui.label(RichText::new(format!("New version {} available (you have v{}).", release.tag, update::CURRENT_VERSION)).strong());
                if let Some(url) = &release.asset_url
                    && ui.button("Download").clicked()
                {
                    ctx.open_url(egui::OpenUrl::new_tab(url));
                }
                if ui.button("What's new").clicked() {
                    ctx.open_url(egui::OpenUrl::new_tab(&release.page_url));
                }
                if ui.small_button("Close").clicked() {
                    self.update = None;
                }
            });
        }
    }

    fn save_card(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        theme::card(ui, |ui| {
            // Wrapped rows: on a narrow window or with Windows scaling the items
            // move to the next line instead of drawing over each other.
            ui.horizontal_wrapped(|ui| {
                ui.add_enabled_ui(self.busy == 0, |ui| {
                    let open = if self.game.is_none() {
                        theme::primary_button(ui, "Open ML save…")
                    } else {
                        egui::Button::new("Open another save…")
                    };
                    if ui.add(open).clicked() {
                        self.open_save(&ctx);
                    }
                });
                if ui.button("Game folder (names/OVR)…").clicked()
                    && let Some(folder) = FileDialog::new().set_title("Choose the game folder (the one with the download folder)").pick_folder()
                {
                    self.settings.game_folder = Some(folder.to_string_lossy().into_owned());
                    self.settings.store();
                    self.load_game_folder(&ctx, folder);
                }
                if ui.button("Import names / EDIT…").clicked()
                    && let Some(path) = FileDialog::new().set_title("Name list (TXT/CSV) or EDIT00000000").pick_file()
                {
                    self.spawn(&ctx, move || {
                        let mut catalog = PlayerCatalog::default();
                        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                        let result = if name.to_uppercase().starts_with("EDIT") {
                            catalog.load_edit(&path)
                        } else {
                            catalog.load_name_list(&path)
                        };
                        Job::Names(result.map(|n| (catalog, n, name)))
                    });
                }
                ui.add_space(12.0);
                ui.label("Currency:");
                let before = self.currency;
                egui::ComboBox::from_id_salt("currency")
                    .selected_text(CURRENCIES[self.currency].0)
                    .show_index(ui, &mut self.currency, CURRENCIES.len(), |i| CURRENCIES[i].0);
                if before != self.currency {
                    self.settings.currency = Some(CURRENCIES[self.currency].0.to_string());
                    self.settings.store();
                    self.fill_finances();
                    self.results_dirty = true;
                }
            });
            let muted = theme::muted(ui);
            // One source per game file (data_*.cpk …): show the count, list them in the tooltip.
            let sources = &self.catalog.sources;
            let names = match sources.len() {
                0 => format!("Player names: {} (built-in list not loaded)", fmt_int(self.catalog.players.len() as i64)),
                1 => format!("Player names: {} from {}", fmt_int(self.catalog.players.len() as i64), sources[0]),
                n => format!("Player names: {} from {n} files", fmt_int(self.catalog.players.len() as i64)),
            };
            let names_tip = sources.join("\n");
            // The file name only; the full path (long on Windows) is in the tooltip.
            let row = ui.horizontal_wrapped(|ui| {
                match &self.game {
                    Some(g) => {
                        let file = g.path.file_name().unwrap_or_default().to_string_lossy();
                        theme::chip(ui, Tone::Ok, format!("{file}  ·  {}  ·  {} clubs", g.game, g.teams.len()));
                    }
                    None => {
                        ui.label(RichText::new("No save opened yet. You can also drop the file on this window.").color(muted));
                    }
                }
                let label = ui.label(RichText::new(names).small().color(muted));
                if !names_tip.is_empty() {
                    label.on_hover_text(names_tip);
                }
            });
            if let Some(g) = &self.game {
                row.response.on_hover_text(g.path.display().to_string());
            }
            if self.game.is_some() {
                ui.horizontal_wrapped(|ui| {
                    ui.strong("Your club:");
                    let mine = self.game.as_ref().unwrap().managed_team;
                    let text = mine.map(|i| self.club_label(i)).unwrap_or_else(|| "— choose —".into());
                    let items = self.club_items.clone();
                    if let Some(index) = search_combo(ui, "my_club", 360.0, &text, &mut self.my_club_filter, &items) {
                        self.set_managed_club(index);
                    }
                    if let Some(i) = self.game.as_ref().unwrap().managed_team {
                        let g = self.game.as_ref().unwrap();
                        ui.label(
                            RichText::new(format!("{}/40 players · {} contracts", g.roster(Source::Club(i)).len(), g.contract_count()))
                                .color(muted),
                        );
                    }
                    ui.label(RichText::new(&self.club_info).color(muted));
                });
            }
        });
    }

    fn tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (tab, title) in [(Tab::Transfer, "Player transfers"), (Tab::Budget, "Budget & manager")] {
                let text = RichText::new(title).strong().size(15.0);
                if ui.add(egui::Button::selectable(self.tab == tab, text).min_size(vec2(150.0, 32.0))).clicked() {
                    self.tab = tab;
                }
            }
        });
    }

    fn bottom_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        let pending = self.game.as_ref().is_some_and(|g| !g.history.is_empty());
        ui.horizontal(|ui| {
            theme::section_title(ui, "Unsaved changes");
            if ui.add_enabled(pending, egui::Button::new("↶ Undo last change")).clicked() {
                let game = self.game.as_mut().unwrap();
                match game.undo() {
                    Ok(text) => self.status = format!("Undone: {text}"),
                    Err(e) => error_dialog("Couldn't do that", &e.to_string()),
                }
                self.after_change();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(pending, theme::secondary_button("Save as…")).clicked()
                    && let Some(path) = FileDialog::new()
                        .set_title("Save as")
                        .set_file_name(self.game.as_ref().unwrap().path.file_name().unwrap_or_default().to_string_lossy())
                        .save_file()
                {
                    self.save(Some(path));
                }
                if ui.add_enabled(pending, theme::primary_button(ui, "Save (with automatic backup)")).clicked() {
                    let name = self.game.as_ref().unwrap().path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    if confirm(
                        "Save",
                        &format!(
                            "Overwrite {name}?\nThe old file is backed up to the FLMLDesk_backup folder. Make sure the \
                             game is closed or on the main menu."
                        ),
                    ) {
                        self.save(None);
                    }
                }
            });
        });
        egui::ScrollArea::vertical().max_height(70.0).id_salt("ops").show(ui, |ui| {
            if let Some(g) = &self.game {
                for op in g.operations() {
                    ui.label(op);
                }
            }
        });
        ui.label(RichText::new(&self.status).color(theme::muted(ui)));
        ui.add_space(2.0);
    }

    fn transfer_tab(&mut self, ui: &mut egui::Ui) {
        let Some(game) = &self.game else {
            ui.label("Open an ML save first.");
            return;
        };
        let mine = game.managed_team;
        ui.horizontal(|ui| {
            ui.label("Search (name/ID):");
            if ui.add(egui::TextEdit::singleline(&mut self.query).desired_width(220.0)).changed() {
                self.results_dirty = true;
            }
            ui.label("From:");
            let mut sources = vec![("All clubs".to_string(), SourceFilter::All)];
            if mine.is_some() {
                sources.push(("My club".into(), SourceFilter::Mine));
            }
            sources.push(("Others (national team, no club)".into(), SourceFilter::Others));
            sources.extend(self.club_items.iter().map(|(l, i)| (l.clone(), SourceFilter::Club(*i))));
            let text = sources.iter().find(|(_, v)| *v == self.source).map(|(l, _)| l.clone()).unwrap_or_default();
            if let Some(source) = search_combo(ui, "source", 320.0, &text, &mut self.source_filter, &sources) {
                self.source = source;
                self.results_dirty = true;
            }
            ui.weak(format!("{} players", fmt_int(self.results.len() as i64)));
        });
        ui.add_space(4.0);

        // Detail form sits at the bottom; the table takes the remaining height.
        egui::Panel::bottom("transfer_form").frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(0, 6))).show(ui, |ui| {
            theme::card(ui, |ui| self.transfer_form(ui))
        });
        egui::CentralPanel::default().show(ui, |ui| self.results_table(ui));
    }

    fn results_table(&mut self, ui: &mut egui::Ui) {
        let mut clicked = None;
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::remainder().at_least(180.0).clip(true))
            .column(Column::exact(48.0))
            .column(Column::exact(48.0))
            .column(Column::exact(48.0))
            .column(Column::initial(220.0).clip(true))
            .column(Column::initial(130.0))
            .column(Column::exact(80.0))
            .header(22.0, |mut header| {
                for title in ["Player", "Pos", "Age", "OVR", "Club", "Salary/yr (your club)", "ID"] {
                    header.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|body| {
                body.rows(20.0, self.results.len(), |mut row| {
                    let r = &self.results[row.index()];
                    row.set_selected(self.selected == Some((r.source, r.player)));
                    row.col(|ui| {
                        ui.label(&r.info.name);
                    });
                    row.col(|ui| {
                        ui.label(r.info.position.unwrap_or(""));
                    });
                    row.col(|ui| {
                        ui.label(r.info.age.map(|a| a.to_string()).unwrap_or_default());
                    });
                    row.col(|ui| {
                        ui.label(r.info.overall.map(|o| o.to_string()).unwrap_or_default());
                    });
                    row.col(|ui| {
                        ui.label(&r.club);
                    });
                    row.col(|ui| {
                        ui.label(&r.wage);
                    });
                    row.col(|ui| {
                        ui.label(r.player.player_id.to_string());
                    });
                    if row.response().clicked() {
                        clicked = Some((r.source, r.player));
                    }
                });
            });
        if let Some((source, player)) = clicked {
            self.select(source, player);
        }
    }

    fn transfer_form(&mut self, ui: &mut egui::Ui) {
        let game = self.game.as_ref().unwrap();
        let player = match self.selected {
            Some((source, player)) => {
                let info = self.catalog.get(player.player_id);
                format!(
                    "{} · {} · age {} · OVR {} · {}",
                    info.name,
                    info.position.unwrap_or("-"),
                    info.age.map_or("-".into(), |a| a.to_string()),
                    info.overall.map_or("-".into(), |o| o.to_string()),
                    game.source_name(source)
                )
            }
            None => "Select a player in the table.".into(),
        };
        let mine = game.managed_team;
        let mut dest_items: Vec<(String, usize)> = Vec::new();
        if let Some(m) = mine {
            dest_items.push((format!("★ {} (my club)", self.club_label(m)), m));
        }
        dest_items.extend(self.club_items.iter().cloned());
        let mode = self.mode();
        let ends: Vec<Date> = (1..=5).map(|n| game.contract_end_in(n)).collect();
        ui.horizontal(|ui| {
            theme::section_title(ui, "Transfer");
            ui.strong(player);
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("To club:");
            let text = self.dest.and_then(|d| dest_items.iter().find(|(_, i)| *i == d)).map(|(l, _)| l.clone()).unwrap_or_default();
            if let Some(d) = search_combo(ui, "dest", 360.0, &text, &mut self.dest_filter, &dest_items) {
                self.dest = Some(d);
            }
        });
        let money = matches!(mode, Some(Mode::In | Mode::Out));
        let incoming = mode == Some(Mode::In);
        ui.horizontal_wrapped(|ui| {
            ui.label(if mode == Some(Mode::Out) { "Fee received:" } else { "Transfer fee:" });
            ui.add_enabled_ui(money, |ui| ui.add(egui::TextEdit::singleline(&mut self.fee).desired_width(130.0)));
            ui.add_space(8.0);
            ui.label("Salary per year:");
            ui.add_enabled_ui(incoming, |ui| ui.add(egui::TextEdit::singleline(&mut self.salary).desired_width(130.0)));
            ui.add_space(8.0);
            ui.label("Contract until:");
            ui.add_enabled_ui(incoming, |ui| {
                ui.add(egui::DragValue::new(&mut self.end_year).range(2000..=2150)).on_hover_text("Year");
                ui.add(egui::DragValue::new(&mut self.end_month).range(1..=12)).on_hover_text("Month");
                ui.add(egui::DragValue::new(&mut self.end_day).range(1..=31)).on_hover_text("Day");
                for (n, end) in ends.iter().enumerate() {
                    let current = (self.end_year, self.end_month, self.end_day) == (end.year, end.month, end.day);
                    if ui.selectable_label(current, format!("+{} yr", n + 1)).on_hover_text(end.to_string()).clicked() {
                        (self.end_year, self.end_month, self.end_day) = (end.year, end.month, end.day);
                    }
                }
            });
        });
        ui.horizontal(|ui| {
            ui.add_enabled_ui(money, |ui| ui.checkbox(&mut self.use_tb, "Pay fee from transfer budget"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let can = self.busy == 0 && mode.is_some();
                if ui.add_enabled(can, theme::primary_button(ui, "Transfer →")).clicked() {
                    self.do_transfer();
                }
            });
        });
        let info = theme::tone_color(ui, Tone::Info);
        for line in self.preview(mode) {
            ui.label(RichText::new(line).color(info));
        }
    }

    fn budget_tab(&mut self, ui: &mut egui::Ui) {
        let Some(game) = &self.game else {
            ui.label("Open an ML save first.");
            return;
        };
        if let Some(e) = &game.finance_error {
            let message = format!("The budget can't be edited: {e}");
            theme::callout(ui, Tone::Warn, |ui| {
                ui.label(message);
            });
            return;
        }
        let saved = game.finances().ok();
        let wages = game.wage_bill_eur();
        let has_club = game.managed_team.is_some();
        let symbol = CURRENCIES[self.currency].0.split(' ').next().unwrap_or("EUR").to_string();
        let rate = self.rate();
        let bump = |field: &mut String, eur: i64| {
            if let Ok(v) = parse_amount(if field.is_empty() { "0" } else { field }) {
                *field = fmt_int(v + (eur as f64 * rate).round_ties_even() as i64);
            }
        };
        ui.add_space(4.0);
        let muted = theme::muted(ui);
        theme::card(ui, |ui| {
            theme::section_title(ui, "Budget & manager");
            egui::Grid::new("budget").num_columns(3).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label(format!("Transfer budget ({symbol}):"));
                ui.add(egui::TextEdit::singleline(&mut self.tb).desired_width(220.0));
                ui.horizontal(|ui| {
                    for m in [10, 50, 100] {
                        if ui.button(format!("+€{m}m")).clicked() {
                            bump(&mut self.tb, m * 1_000_000);
                        }
                    }
                });
                ui.end_row();
                ui.label(format!("Salary budget ({symbol}):"));
                ui.add(egui::TextEdit::singleline(&mut self.sb).desired_width(220.0));
                ui.horizontal(|ui| {
                    for m in [5, 20] {
                        if ui.button(format!("+€{m}m")).clicked() {
                            bump(&mut self.sb, m * 1_000_000);
                        }
                    }
                });
                ui.end_row();
                ui.label("Manager name:");
                ui.add(egui::TextEdit::singleline(&mut self.manager).desired_width(220.0));
                ui.end_row();
            });
            ui.add_space(8.0);
            if ui.add_enabled(self.busy == 0, theme::primary_button(ui, "Apply")).clicked() {
                self.apply_finances();
            }
            ui.add_space(8.0);
            ui.label(RichText::new("Both amounts are as shown in the game. You can type amounts like 12,500,000, 12.5m or 750k; they are saved in steps of €100.").color(muted));
            if let Some(f) = saved {
                if has_club {
                    ui.label(
                        RichText::new(format!(
                            "The save stores a yearly salary budget of {}; the game shows it minus your club's salaries ({}).",
                            self.to_display(f.salary_eur),
                            self.to_display(wages)
                        ))
                        .color(muted),
                    );
                } else {
                    theme::chip(ui, Tone::Warn, "Choose your club first: until then the salary budget can't be shown as in the game.");
                }
            }
        });
    }
}

/// The window icon, shown again in the header.
fn logo_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let icon = eframe::icon_data::from_png_bytes(crate::ICON_PNG).expect("bundled icon is a valid PNG");
    let image = egui::ColorImage::from_rgba_unmultiplied([icon.width as usize, icon.height as usize], &icon.rgba);
    ctx.load_texture("logo", image, egui::TextureOptions::LINEAR)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_jobs();
        let dropped = ui.ctx().input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()).filter(|p| p.is_file()));
        if let Some(path) = dropped
            && self.busy == 0
        {
            let ctx = ui.ctx().clone();
            self.open_path(&ctx, path);
        }
        if self.results_dirty {
            self.refresh_results();
        }
        egui::Panel::top("top").frame(egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(14, 10))).show(ui, |ui| {
            self.header(ui);
            ui.add_space(6.0);
            self.save_card(ui);
            ui.add_space(6.0);
            self.tabs(ui);
        });
        egui::Panel::bottom("bottom").frame(egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(14, 6))).show(ui, |ui| self.bottom_bar(ui));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(14, 4))).show(ui, |ui| match self.tab {
            Tab::Transfer => self.transfer_tab(ui),
            Tab::Budget => self.budget_tab(ui),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_accept_grouped_and_short_forms() {
        assert_eq!(parse_amount("12,500,000"), Ok(12_500_000));
        assert_eq!(parse_amount("12,500k"), Ok(12_500_000));
        assert_eq!(parse_amount("12.5 M"), Ok(12_500_000));
        assert_eq!(parse_amount("750k"), Ok(750_000));
        assert_eq!(parse_amount("2k"), Ok(2_000));
        assert!(parse_amount("").is_err());
        assert!(parse_amount("-5m").is_err());
        assert!(parse_amount("12.500.000").is_err());
        assert!(parse_amount("12a").is_err());
    }

    #[test]
    fn search_ignores_case_and_accents() {
        assert_eq!(normal("Edin DŽEKO"), "edin dzeko");
        assert_eq!(normal("Müller"), "muller");
    }
}
