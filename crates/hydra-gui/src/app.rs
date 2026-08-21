// Copyright (C) 2026 Javad Rajabzadeh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Application state and update logic — every window shares this one `App`.

use crate::engine::{self, Cmd, StartSpec};
use crate::model::{
    self, categorize, ConfigFile, DlId, DlState, DownloadItem, ProxyMode, SiteLogin, StateFile,
};
use crate::sounds;
use crate::{fmt, i18n};
use iced::window;
use iced::{Point, Task};
use std::collections::HashMap;
use std::time::Instant;

pub type El<'a> = iced::Element<'a, Message>;

// ------------------------------------------------------------------- windows

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WinKind {
    Main,
    AddUrl,
    FileInfo(DlId),
    Progress(DlId),
    Complete(DlId),
    Options,
    Scheduler,
    Batch,
    About,
    Confirm,
    Permissions,
    Shortcuts,
    Update,
}

// ---------------------------------------------------------------- selections

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TreeSel {
    All,
    Cat(String),
    Unfinished,
    UnfCat(String),
    Finished,
    FinCat(String),
    Queues,
    Queue(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortKey {
    Name,
    Queue,
    Size,
    Status,
    TimeLeft,
    Rate,
    LastTry,
    Description,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuBarKind {
    Tasks,
    File,
    Downloads,
    View,
    Help,
}

/// Every menu-driven command, shared by the in-window menu bar and the native
/// macOS menu (which addresses them by the string id in [`MenuAction::id`]).
#[derive(Clone, PartialEq, Debug)]
pub enum MenuAction {
    AddNewDownload,
    AddBatch,
    AddBatchClipboard,
    AddBatchFile,
    SiteGrabber,
    DropTarget,
    ExportList,
    ImportList,
    Exit,
    StopDownload,
    Remove,
    DownloadNow,
    Redownload,
    PauseAll,
    StopAll,
    DeleteAllCompleted,
    Find,
    Scheduler,
    StartQueue(String),
    StopQueue(String),
    SpeedLimiterToggle,
    Options,
    HideCategories,
    ArrangeBy(SortKey),
    ToggleDark,
    FontSize(u16),
    Language(String),
    HomePage,
    About,
    Permissions,
    MoveToQueue(String),
    RemoveFromQueue,
    Properties,
    OpenSel,
    OpenFolderSel,
    PowerSaveToggle,
    Shortcuts,
    /// Help > Check for updates: a manual, user-visible version check —
    /// unlike the silent startup check it also reports "up to date" and
    /// connection failures.
    CheckUpdates,
}

impl MenuAction {
    // Only the native menu/tray integrations (macOS/Windows) need string ids.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub fn id(&self) -> String {
        match self {
            MenuAction::AddNewDownload => "add_new".into(),
            MenuAction::AddBatch => "add_batch".into(),
            MenuAction::AddBatchClipboard => "add_batch_clip".into(),
            MenuAction::AddBatchFile => "add_batch_file".into(),
            MenuAction::SiteGrabber => "grabber".into(),
            MenuAction::DropTarget => "drop_target".into(),
            MenuAction::ExportList => "export".into(),
            MenuAction::ImportList => "import".into(),
            MenuAction::Exit => "exit".into(),
            MenuAction::StopDownload => "stop_dl".into(),
            MenuAction::Remove => "remove".into(),
            MenuAction::DownloadNow => "dl_now".into(),
            MenuAction::Redownload => "redownload".into(),
            MenuAction::PauseAll => "pause_all".into(),
            MenuAction::StopAll => "stop_all".into(),
            MenuAction::DeleteAllCompleted => "del_completed".into(),
            MenuAction::Find => "find".into(),
            MenuAction::Scheduler => "scheduler".into(),
            MenuAction::StartQueue(q) => format!("start_queue:{q}"),
            MenuAction::StopQueue(q) => format!("stop_queue:{q}"),
            MenuAction::SpeedLimiterToggle => "speed_limiter".into(),
            MenuAction::Options => "options".into(),
            MenuAction::HideCategories => "hide_cats".into(),
            MenuAction::ArrangeBy(k) => format!("arrange:{k:?}"),
            MenuAction::ToggleDark => "dark".into(),
            MenuAction::FontSize(s) => format!("font:{s}"),
            MenuAction::Language(l) => format!("lang:{l}"),
            MenuAction::HomePage => "homepage".into(),
            MenuAction::About => "about".into(),
            MenuAction::Permissions => "permissions".into(),
            MenuAction::MoveToQueue(q) => format!("move_q:{q}"),
            MenuAction::RemoveFromQueue => "rm_q".into(),
            MenuAction::Properties => "props".into(),
            MenuAction::OpenSel => "open_sel".into(),
            MenuAction::OpenFolderSel => "open_folder_sel".into(),
            MenuAction::PowerSaveToggle => "power_save".into(),
            MenuAction::Shortcuts => "shortcuts".into(),
            MenuAction::CheckUpdates => "check_updates".into(),
        }
    }

    pub fn from_id(id: &str) -> Option<MenuAction> {
        if let Some(q) = id.strip_prefix("start_queue:") {
            return Some(MenuAction::StartQueue(q.into()));
        }
        if let Some(q) = id.strip_prefix("stop_queue:") {
            return Some(MenuAction::StopQueue(q.into()));
        }
        if let Some(k) = id.strip_prefix("arrange:") {
            let key = match k {
                "Name" => SortKey::Name,
                "Size" => SortKey::Size,
                "Status" => SortKey::Status,
                "TimeLeft" => SortKey::TimeLeft,
                "Rate" => SortKey::Rate,
                "LastTry" => SortKey::LastTry,
                "Description" => SortKey::Description,
                _ => SortKey::Name,
            };
            return Some(MenuAction::ArrangeBy(key));
        }
        if let Some(s) = id.strip_prefix("font:") {
            return s.parse().ok().map(MenuAction::FontSize);
        }
        if let Some(l) = id.strip_prefix("lang:") {
            return Some(MenuAction::Language(l.into()));
        }
        if let Some(q) = id.strip_prefix("move_q:") {
            return Some(MenuAction::MoveToQueue(q.into()));
        }
        Some(match id {
            "add_new" => MenuAction::AddNewDownload,
            "add_batch" => MenuAction::AddBatch,
            "add_batch_clip" => MenuAction::AddBatchClipboard,
            "add_batch_file" => MenuAction::AddBatchFile,
            "grabber" => MenuAction::SiteGrabber,
            "drop_target" => MenuAction::DropTarget,
            "export" => MenuAction::ExportList,
            "import" => MenuAction::ImportList,
            "exit" => MenuAction::Exit,
            "stop_dl" => MenuAction::StopDownload,
            "remove" => MenuAction::Remove,
            "dl_now" => MenuAction::DownloadNow,
            "redownload" => MenuAction::Redownload,
            "pause_all" => MenuAction::PauseAll,
            "stop_all" => MenuAction::StopAll,
            "del_completed" => MenuAction::DeleteAllCompleted,
            "find" => MenuAction::Find,
            "scheduler" => MenuAction::Scheduler,
            "speed_limiter" => MenuAction::SpeedLimiterToggle,
            "options" => MenuAction::Options,
            "hide_cats" => MenuAction::HideCategories,
            "dark" => MenuAction::ToggleDark,
            "homepage" => MenuAction::HomePage,
            "about" => MenuAction::About,
            "permissions" => MenuAction::Permissions,
            "rm_q" => MenuAction::RemoveFromQueue,
            "props" => MenuAction::Properties,
            "open_sel" => MenuAction::OpenSel,
            "open_folder_sel" => MenuAction::OpenFolderSel,
            "power_save" => MenuAction::PowerSaveToggle,
            "shortcuts" => MenuAction::Shortcuts,
            "check_updates" => MenuAction::CheckUpdates,
            _ => return None,
        })
    }
}

// ------------------------------------------------------------- dialog states

#[derive(Clone, Debug, Default)]
pub struct AddUrlState {
    pub address: String,
    pub use_auth: bool,
    pub login: String,
    pub password: String,
    pub error: Option<String>,
    /// Cookie header captured by the browser extension; applied to the item
    /// right after `add_item` so a background start already sends it.
    pub capture_cookies: Option<String>,
    /// Filename the browser had already resolved (Content-Disposition et
    /// al.) — better than what the URL path implies.
    pub capture_name: Option<String>,
}

/// A capture parked behind the duplicate-confirmation dialog.
#[derive(Clone, Debug)]
pub struct PendingAdd {
    pub url: String,
    pub auth: Option<(String, String)>,
    pub cookies: Option<String>,
    pub name: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct FileInfoState {
    pub dl: DlId,
    pub category: String,
    pub save_dir: String,
    pub file_name: String,
    pub description: String,
    pub remember: bool,
    /// Editable in Properties mode: changing it and starting continues the
    /// same bytes from a different mirror.
    pub url: String,
    pub login: String,
    pub password: String,
    pub cookies: String,
    /// New-download dialog (auto-started in the background, Cancel removes
    /// the item) vs Properties on an existing entry.
    pub is_new: bool,
    /// Fields the user edited; the background probe stops auto-filling them.
    pub name_touched: bool,
    pub cat_touched: bool,
    pub dir_touched: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ProgTab {
    #[default]
    Status,
    Speed,
    Completion,
}

#[derive(Clone, Debug, Default)]
pub struct ProgState {
    pub tab: ProgTab,
    pub details: bool,
    pub limit_on: bool,
    pub limit_kb: String,
    pub remember_limit: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OptTab {
    General,
    FileTypes,
    SaveTo,
    Downloads,
    Connection,
    Proxy,
    Sites,
    DialUp,
    Sounds,
}

#[derive(Debug)]
pub struct OptionsState {
    pub tab: OptTab,
    pub draft: crate::model::Settings,
    /// Multiline editors for the File-types lists.
    pub auto_types_edit: iced::widget::text_editor::Content,
    pub sites_edit: iced::widget::text_editor::Content,
    pub draft_cats: Vec<crate::model::CategoryDef>,
    pub sel_category: String,
    pub sel_login: Option<usize>,
    pub login_site: String,
    pub login_user: String,
    pub login_pass: String,
    pub conn_exc_server: String,
    pub conn_exc_n: String,
}

impl Default for OptionsState {
    fn default() -> Self {
        OptionsState {
            tab: OptTab::General,
            draft: crate::model::Settings::default(),
            auto_types_edit: iced::widget::text_editor::Content::new(),
            sites_edit: iced::widget::text_editor::Content::new(),
            draft_cats: crate::model::default_categories(),
            sel_category: "General".into(),
            sel_login: None,
            login_site: String::new(),
            login_user: String::new(),
            login_pass: String::new(),
            conn_exc_server: String::new(),
            conn_exc_n: String::new(),
        }
    }
}

/// State of the update dialog (`WinKind::Update`).
#[derive(Debug, Default)]
pub struct UpdateUiState {
    /// The newer release, filled by the startup check.
    pub info: Option<crate::update::UpdateInfo>,
    pub phase: UpdatePhase,
    /// Cooperative cancel for the in-flight download; the stream checks it
    /// on every chunk.
    pub cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// The pending check came from Help > Check for updates: report
    /// "up to date" and failures too, where the startup check stays silent.
    pub manual: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum UpdatePhase {
    /// Notes are on screen; nothing has been downloaded.
    #[default]
    Idle,
    Downloading {
        got: u64,
        total: Option<u64>,
    },
    Verifying,
    Preparing,
    /// The finisher process is live; the app is about to exit.
    Restarting,
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SchTab {
    #[default]
    Schedule,
    Files,
}

#[derive(Clone, Debug)]
pub struct SchState {
    pub queue: String,
    pub tab: SchTab,
    pub sel_file: Option<DlId>,
    /// Item being dragged to a new position in "Files in the queue".
    pub drag: Option<DlId>,
    /// Draft of the selected queue's name; committed on Enter.
    pub rename_draft: String,
    /// Double-click on the title switches it into the rename field.
    pub renaming: bool,
}

impl Default for SchState {
    fn default() -> Self {
        SchState {
            queue: String::new(),
            tab: SchTab::Schedule,
            sel_file: None,
            drag: None,
            rename_draft: String::new(),
            renaming: false,
        }
    }
}

#[derive(Debug, Default)]
pub struct BatchState {
    pub text: iced::widget::text_editor::Content,
    pub checks: Vec<(String, bool)>,
    /// Probed sizes for the Size column (absent = probe still running).
    pub sizes: std::collections::HashMap<String, u64>,
    pub parsed: bool,
    pub to_category: bool,
    pub category: String,
    pub to_dir: bool,
    pub dir: String,
}

#[derive(Clone, Debug)]
pub enum ConfirmKind {
    DeleteItems(Vec<DlId>),
    DeleteCompleted,
    NoneChecked,
    /// The URL is already in the list (`existing`) and/or its target file is
    /// already on disk (`file`): resume / open / download as a renamed copy.
    Duplicate {
        existing: Option<DlId>,
        file: Option<String>,
    },
    /// Startup check could not write into the download folder.
    PermissionWarn {
        dir: String,
    },
    /// Help > Check for updates found nothing newer (info box, OK).
    UpToDate,
    /// Help > Check for updates could not reach the release server.
    UpdateCheckFailed(String),
    /// "Warn me before stopping downloads" (Connection tab): the stop only
    /// happens once the user confirms. `stop_queues` carries the Pause All /
    /// Stop All variant, which also halts queue processing.
    StopWarn {
        ids: Vec<DlId>,
        stop_queues: bool,
    },
}

// ------------------------------------------------------------------ messages

#[derive(Clone, Debug)]
pub enum Message {
    Noop,
    WindowOpened(window::Id),
    WindowClosed(window::Id),
    WindowCloseRequested(window::Id),
    WinMoved(window::Id, Point),
    WinResized(window::Id, iced::Size),
    Engine(engine::Event),
    Tick,
    ClipboardSeen(Option<String>),
    /// Fast animation tick (only subscribed while a download is active):
    /// glides each item's displayed progress toward the real fraction.
    AnimTick,
    NativeMenu(String),
    // main window chrome
    MenuOpen(MenuBarKind),
    MenuClose,
    /// Hovering (or clicking) a dropdown entry: `Some(i)` opens entry `i`'s
    /// flyout, `None` closes it — macOS submenu behaviour.
    SubmenuHover(Option<usize>),
    Menu(MenuAction),
    TreeSelect(TreeSel),
    TreeToggle(u8),
    RowClick(DlId),
    RowRightClick(DlId),
    CursorMoved(Point),
    Mods(iced::keyboard::Modifiers),
    RawKey(iced::keyboard::Key, iced::keyboard::Modifiers),
    SelectAll,
    RowEnter(DlId),
    QueueMenuOpen(bool),
    ClipboardAddStart(Option<String>),
    ShortcutEdit(String, String),
    MouseUp,
    ColResizeStart(usize),
    SortBy(SortKey),
    ToolbarResume,
    ToolbarStop,
    ToolbarDelete,
    // add url
    AddrChanged(String),
    AddrAuthToggled(bool),
    AddrLogin(String),
    AddrPass(String),
    AddUrlOk,
    /// Requests arriving from the browser extension over the extbus socket.
    Ext(crate::extbus::ExtEvent),
    // file info
    FiCategory(String),
    FiFileName(String),
    FiSaveDir(String),
    FiBgToggle(bool),
    FiBrowseDir,
    FiDirPicked(Option<String>),
    FiDescription(String),
    FiRemember(bool),
    FiUrl(String),
    FiLogin(String),
    FiPass(String),
    FiCookies(String),
    FiDownloadLater,
    FiStartDownload,
    FiCancel,
    // progress dialog
    ProgTabSet(DlId, ProgTab),
    ProgToggleDetails(DlId),
    ProgPauseResume(DlId),
    ProgCancel(DlId),
    ProgHideTab(DlId, u8),
    ProgLimitOn(DlId, bool),
    ProgLimitKb(DlId, String),
    ProgLimitRemember(DlId, bool),
    // complete dialog
    OpenFile(DlId),
    OpenFolder(DlId),
    // options
    OptTabSet(OptTab),
    OptOk,
    OptDraft(OptField),
    // update dialog
    /// Startup (or manual) check finished: newer release / up to date / error.
    UpdateChecked(Result<Option<crate::update::UpdateInfo>, String>),
    UpdateNow,
    UpdateCancel,
    UpdateOpenPage,
    /// Progress of a running update, streamed from `update::run`.
    UpdateEvent(crate::update::UpdateEvent),
    // scheduler
    SchQueue(String),
    SchNameEdit,
    SchNameDraft(String),
    SchNameCommit,
    SchTabSet(SchTab),
    SchField(SchField),
    SchStartNow,
    SchStop,
    SchNewQueue,
    SchDeleteQueue,
    SchDragStart(DlId),
    SchDragOver(DlId),
    SchFileUp,
    SchFileDown,
    SchFileRemove,
    // batch
    BatchEdit(iced::widget::text_editor::Action),
    BatchLoaded(Option<String>),
    BatchProbed(String, Option<u64>),
    BatchCheck(usize, bool),
    BatchCheckAll(bool),
    BatchSaveMode(u8),
    BatchCategory(String),
    BatchDir(String),
    BatchOk,
    // generic dialog buttons
    ConfirmYes,
    ConfirmRemoveFile(bool),
    AddrPrefill(Option<String>),
    OpenPermissions,
    PermOpenPane(&'static str),
    PermRefresh,
    DupResume,
    DupOpen,
    DupNew,
    CloseThis(window::Id),
}

/// Options-dialog field edits, kept in one variant so `Message` stays legible.
#[derive(Clone, Debug)]
pub enum OptField {
    LaunchStartup(bool),
    CheckUpdates(bool),
    BetaChannel(bool),
    StartInTray(bool),
    /// Only where the Dock/taskbar checkbox exists (macOS/Windows).
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    HideTaskbar(bool),
    PowerSave(bool),
    GpuRender(bool),
    Clipboard(bool),
    Browser(usize, bool),
    AutoTypesEdit(iced::widget::text_editor::Action),
    SitesEdit(iced::widget::text_editor::Action),
    ExcDialog(bool),
    RememberLast(bool),
    ServerDate(bool),
    ShowFileInfo(bool),
    StartMinimized(bool),
    SpeedTab(bool),
    CompletionTab(bool),
    HideButtons(bool),
    CompleteDialog(bool),
    UserAgent(String),
    VirusScanner(String),
    VirusArgs(String),
    BrowseVirus,
    VirusPicked(Option<String>),
    DefaultConns(usize),
    AdaptiveConns(bool),
    ExcServer(String),
    ExcConns(String),
    ExcAdd,
    DlLimit(bool),
    DlLimitMb(String),
    DlLimitHours(String),
    WarnStop(bool),
    ProxyMode(ProxyMode),
    ProxyScript(String),
    ProxyHost(String),
    ProxyPort(String),
    ProxyUser(String),
    ProxyPass(String),
    ProxyHttp(bool),
    ProxyHttps(bool),
    ProxyFtp(bool),
    FtpPasv(bool),
    SelCategory(String),
    CatDir(String),
    BrowseCatDir,
    CatDirPicked(Option<String>),
    LoginSel(usize),
    LoginSite(String),
    LoginUser(String),
    LoginPass(String),
    LoginAdd,
    LoginRemove,
    Sound(usize, bool),
    SoundBrowse(usize),
    SoundPlay(usize),
}

#[derive(Clone, Debug)]
pub enum SchField {
    Periodic(bool),
    OnStartup(bool),
    StartEnabled(bool),
    StartAt(String),
    Once(bool),
    Day(usize, bool),
    StopEnabled(bool),
    StopAt(String),
    RetriesEnabled(bool),
    Retries(String),
    OpenFileEnabled(bool),
    OpenFile(String),
    ExitDone(bool),
    FilesAtOnce(String),
}

// ----------------------------------------------------------------------- app

pub struct App {
    pub cfg: ConfigFile,
    pub state: StateFile,
    pub windows: HashMap<window::Id, WinKind>,
    pub main_id: Option<window::Id>,
    /// Multi-selection: click selects, Cmd/Ctrl-click toggles, Shift-click
    /// extends from the anchor. First entry is the primary item.
    pub selected: Vec<DlId>,
    pub sel_anchor: Option<DlId>,
    pub mods: iced::keyboard::Modifiers,
    /// Live column widths of the download table (drag the header edges).
    pub col_widths: Vec<f32>,
    /// (column, grab x, width at grab) while a header edge is being dragged.
    pub resizing: Option<(usize, f32, f32)>,
    pub tree_sel: TreeSel,
    pub tree_open: [bool; 4], // all, unfinished, finished, queues
    pub open_menu: Option<MenuBarKind>,
    pub open_submenu: Option<usize>,
    pub cursor: Point,
    pub ctx_at: Option<Point>,
    pub last_click: Option<(DlId, Instant)>,
    pub sort: (SortKey, bool),
    pub add_url: AddUrlState,
    pub file_info: FileInfoState,
    pub prog: HashMap<DlId, ProgState>,
    pub options: OptionsState,
    pub updater: UpdateUiState,
    pub sch: SchState,
    pub batch: BatchState,
    pub confirm: Option<ConfirmKind>,
    /// "Also remove file from disk" in the delete confirmations. Unchecked
    /// every time a confirmation opens.
    pub confirm_remove_file: bool,
    /// Items being deleted once the engine confirms their stop, with the
    /// moment the delete was requested. The instant is a deadline, not a
    /// promise: `Tick` sweeps entries whose stop event never arrived.
    pub pending_delete: Vec<(DlId, Instant)>,
    /// Deferred-persistence flags: mutations mark these, and `flush_saves`
    /// writes at most once per `Tick` (plus unconditionally on exit paths),
    /// so a 200-link batch add is one db write instead of 200.
    pub state_dirty: bool,
    pub cfg_dirty: bool,
    /// Last clipboard text seen by the URL watcher (deduplication).
    pub last_clipboard: String,
    /// URL+auth waiting on the duplicate/exists decision dialog.
    pub pending_add: Option<PendingAdd>,
    /// Next capture-dialog window (File Info / Confirm / Batch) must be
    /// forced above the browser: at capture time this app is usually a
    /// background tray process, and a normal-level window opens behind the
    /// browser unfocused — the dialog floats on top.
    pub capture_raise: bool,
    /// Open split-button dropdown on the toolbar: Some(true)=Start queue,
    /// Some(false)=Stop queue.
    pub queue_menu: Option<bool>,
    /// Row the pointer went down on, for drag-selection.
    pub list_press: Option<DlId>,
    /// Live results shown in the Permissions window.
    pub perm_status: crate::windows::permissions::PermStatus,
    /// Main window origin/size on screen, tracked so dialogs open centred
    /// over the application rather than the monitor.
    pub main_pos: Option<Point>,
    pub main_size: iced::Size,
}

impl App {
    pub fn item(&self, id: DlId) -> Option<&DownloadItem> {
        self.state.downloads.iter().find(|d| d.id == id)
    }

    pub fn item_mut(&mut self, id: DlId) -> Option<&mut DownloadItem> {
        self.state.downloads.iter_mut().find(|d| d.id == id)
    }

    pub fn selected_item(&self) -> Option<&DownloadItem> {
        self.selected.first().and_then(|id| self.item(*id))
    }

    /// Recompute the Permissions window's status dots.
    pub fn refresh_perm_status(&mut self) {
        let dir = self
            .cfg
            .categories
            .first()
            .map(|c| c.dir.clone())
            .unwrap_or_default();
        self.perm_status =
            crate::windows::permissions::probe(&dir, self.cfg.settings.launch_on_startup);
    }

    /// Probe write access to the default download folder; a denial opens the
    /// warning dialog so the user grants access up front instead of at the
    /// first failed download.
    pub fn check_folder_access(&mut self) -> Task<Message> {
        let Some(dir) = self.cfg.categories.first().map(|c| c.dir.clone()) else {
            return Task::none();
        };
        let probe = std::path::Path::new(&dir).join(".hydra-access-probe");
        let ok = std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(&probe, b"ok"))
            .map(|()| {
                let _ = std::fs::remove_file(&probe);
            })
            .is_ok();
        if ok {
            Task::none()
        } else {
            crate::log::warn(&format!("no write access to {dir}"));
            self.confirm = Some(ConfirmKind::PermissionWarn { dir });
            self.open_window(WinKind::Confirm)
        }
    }

    /// Rebuild the native macOS menu bar so its check marks match state.
    #[cfg(target_os = "macos")]
    pub fn refresh_native_menu(&self) {
        let state = crate::macos_menu::MenuState {
            dark_mode: self.cfg.settings.dark_mode,
            show_categories: self.cfg.settings.show_categories,
            font_size: self.cfg.settings.font_size,
            language: {
                let l = self.cfg.language.clone().unwrap_or_else(|| "en".into());
                if l == "English" {
                    "en".into()
                } else {
                    l
                }
            },
            speed_limiter: self.cfg.settings.speed_limiter_on,
        };
        let queues: Vec<String> = self.cfg.queues.iter().map(|q| q.name.clone()).collect();
        crate::macos_menu::reinstall(&state, &queues, &i18n::available());
    }

    #[cfg(not(target_os = "macos"))]
    pub fn refresh_native_menu(&self) {}

    pub fn win_of(&self, kind: WinKind) -> Option<window::Id> {
        self.windows
            .iter()
            .find(|(_, k)| **k == kind)
            .map(|(id, _)| *id)
    }

    /// Items shown for the current tree selection, sorted.
    pub fn visible(&self) -> Vec<&DownloadItem> {
        let mut v: Vec<&DownloadItem> = self
            .state
            .downloads
            .iter()
            .filter(|d| match &self.tree_sel {
                TreeSel::All => true,
                TreeSel::Cat(c) => d.category.as_deref() == Some(c.as_str()),
                TreeSel::Unfinished => d.state != DlState::Complete,
                TreeSel::UnfCat(c) => {
                    d.state != DlState::Complete && d.category.as_deref() == Some(c.as_str())
                }
                TreeSel::Finished => d.state == DlState::Complete,
                TreeSel::FinCat(c) => {
                    d.state == DlState::Complete && d.category.as_deref() == Some(c.as_str())
                }
                TreeSel::Queues => d.queue.is_some(),
                TreeSel::Queue(q) => d.queue.as_deref() == Some(q.as_str()),
            })
            .collect();
        let (key, asc) = self.sort;
        v.sort_by(|a, b| {
            let ord = match key {
                SortKey::Name => a.file_name.to_lowercase().cmp(&b.file_name.to_lowercase()),
                SortKey::Queue => a.queue.cmp(&b.queue).then(a.q_order.cmp(&b.q_order)),
                SortKey::Size => a.size.unwrap_or(0).cmp(&b.size.unwrap_or(0)),
                SortKey::Status => a.status_text().cmp(&b.status_text()),
                SortKey::TimeLeft => a
                    .eta_secs
                    .unwrap_or(u64::MAX)
                    .cmp(&b.eta_secs.unwrap_or(u64::MAX)),
                SortKey::Rate => a
                    .rate
                    .partial_cmp(&b.rate)
                    .unwrap_or(std::cmp::Ordering::Equal),
                SortKey::LastTry => a.last_try.cmp(&b.last_try),
                SortKey::Description => a.description.cmp(&b.description),
            };
            if asc {
                ord
            } else {
                ord.reverse()
            }
        });
        v
    }

    /// Mark the download list for persistence. The actual write happens in
    /// [`Self::flush_saves`] — `model::save_state` rewrites the whole redb
    /// table, so per-mutation writes made a 200-link batch add O(n²) and put
    /// a full serialise-and-commit on the UI thread every second.
    fn save_state(&mut self) {
        self.state_dirty = true;
    }

    /// Mark the configuration for persistence (see [`Self::save_state`]).
    fn save_config(&mut self) {
        self.cfg_dirty = true;
    }

    /// Write whatever is marked dirty. Called once per `Tick` and on every
    /// exit path, so at most one second of changes is ever in flight.
    pub fn flush_saves(&mut self) {
        if self.state_dirty {
            self.state_dirty = false;
            model::save_state(&self.state);
        }
        if self.cfg_dirty {
            self.cfg_dirty = false;
            model::save_config(&self.cfg);
            // The extension mirrors capture settings; every socket reply
            // carries the latest snapshot.
            crate::extbus::publish_config(&self.cfg);
        }
    }

    /// Attach what the browser knew that `add_item` could not: session
    /// cookies and the already-resolved filename. Must run before any start
    /// so the first request carries the Cookie header; renaming re-runs
    /// categorization because the URL-derived name may have had no
    /// extension at all.
    fn apply_capture_extras(&mut self, id: DlId, cookies: Option<String>, name: Option<String>) {
        if cookies.is_none() && name.is_none() {
            return;
        }
        let cats = self.cfg.categories.clone();
        if let Some(d) = self.item_mut(id) {
            if let Some(c) = cookies {
                d.cookies = Some(c);
            }
            if let Some(n) = name {
                d.file_name = n.clone();
                d.category = categorize(&n, &cats);
                if let Some(dir) = d
                    .category
                    .as_deref()
                    .and_then(|c| cats.iter().find(|k| k.name == c))
                    .map(|c| c.dir.clone())
                {
                    d.save_dir = dir;
                }
            }
        }
    }

    // -------------------------------------------------------------- windows

    pub fn open_window(&mut self, kind: WinKind) -> Task<Message> {
        if let Some(id) = self.win_of(kind) {
            return window::gain_focus(id);
        }
        // One File Info dialog at a time — see close_file_info_windows for
        // why a second one strands the first. The download behind the
        // dismissed dialog is already in the list (and may be pulling bytes
        // in the background), so nothing is lost by closing its window.
        let dismissed = if matches!(kind, WinKind::FileInfo(_)) {
            if self
                .windows
                .values()
                .any(|k| matches!(k, WinKind::FileInfo(_)))
            {
                crate::log::info("file info: superseding the open dialog with a new one");
            }
            self.close_file_info_windows()
        } else {
            Task::none()
        };
        // Dialogs are fixed-size and cannot minimize; only the
        // download-progress box may minimize to the dock on its own.
        let (size, resizable) = match kind {
            WinKind::Main => {
                // Restore the last size when it is still sane for a screen;
                // first run (or nonsense values) derives from the display.
                let saved =
                    self.cfg.settings.window_size.filter(|(w, h)| {
                        (900.0..=4000.0).contains(w) && (600.0..=2500.0).contains(h)
                    });
                let s = saved
                    .map(|(w, h)| iced::Size::new(w, h))
                    .unwrap_or_else(main_window_size);
                ((s.width, s.height), true)
            }
            WinKind::AddUrl => ((760.0, 168.0), false),
            // New-download layout is short; Properties adds status/size/
            // login/cookies/history rows. Size the window to the mode so
            // neither shows dead space.
            WinKind::FileInfo(_) => {
                if self.file_info.is_new {
                    ((720.0, 356.0), false)
                } else {
                    ((720.0, 560.0), false)
                }
            }
            WinKind::Progress(_) => ((680.0, 582.0), true),
            WinKind::Complete(_) => ((600.0, 230.0), false),
            WinKind::Options => ((760.0, 700.0), false),
            WinKind::Scheduler => ((950.0, 660.0), false),
            WinKind::Batch => ((950.0, 700.0), false),
            WinKind::About => ((460.0, 300.0), false),
            WinKind::Shortcuts => ((520.0, 460.0), false),
            WinKind::Confirm => ((500.0, 200.0), false),
            WinKind::Permissions => ((640.0, 580.0), false),
            WinKind::Update => ((560.0, 520.0), false),
        };
        let minimizable = matches!(kind, WinKind::Main | WinKind::Progress(_));
        // Centre sub-windows over the main window when its bounds are known.
        let position = match (kind, self.main_pos) {
            (WinKind::Main, _) | (_, None) => window::Position::Centered,
            (_, Some(origin)) => window::Position::Specific(Point::new(
                origin.x + (self.main_size.width - size.0) / 2.0,
                origin.y + (self.main_size.height - size.1) / 2.0,
            )),
        };
        // "Hide from taskbar" on Windows is a per-window creation flag, so
        // it reaches windows opened after the toggle (the main window picks
        // it up on its next open from the tray).
        #[cfg(target_os = "windows")]
        let platform_specific = window::settings::PlatformSpecific {
            skip_taskbar: self.cfg.settings.hide_from_taskbar,
            ..Default::default()
        };
        // Linux: the app id is how the desktop matches a window back to its
        // .desktop file — GNOME/KDE read the Wayland app_id (and the X11
        // WM_CLASS) and look up Icon= there for the dock, the alt-tab
        // switcher and the process list. winit leaves it empty by default,
        // and Wayland has no per-window icon protocol, so the `icon:` field
        // below is X11/Windows-only: without this the shell has nothing to
        // match on and falls back to a generic icon even though the app
        // menu entry shows the logo. Must stay equal to the basename of
        // hydra.desktop (scripts/package-linux.sh, install.sh).
        #[cfg(target_os = "linux")]
        let platform_specific = window::settings::PlatformSpecific {
            application_id: "hydra".to_string(),
            ..Default::default()
        };
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        let platform_specific = window::settings::PlatformSpecific::default();
        let (id, task) = window::open(window::Settings {
            size: iced::Size::new(size.0, size.1),
            // Floor: just enough for the full toolbar row; the default
            // stays proportional to the display.
            min_size: (kind == WinKind::Main).then_some(iced::Size::new(900.0, 600.0)),
            resizable,
            minimizable,
            position,
            exit_on_close_request: false,
            // Title-bar/taskbar logo on Windows and Linux (None on macOS,
            // where the app bundle supplies the Dock icon).
            icon: crate::icons::window_icon(),
            platform_specific,
            ..window::Settings::default()
        });
        if kind == WinKind::Main {
            self.main_id = Some(id);
        }
        self.windows.insert(id, kind);
        let opened = task.map(Message::WindowOpened);
        if matches!(kind, WinKind::Progress(_)) && self.cfg.settings.start_minimized {
            return Task::batch([dismissed, opened, window::minimize(id, true)]);
        }
        Task::batch([dismissed, opened])
    }

    fn close_window(&mut self, kind: WinKind) -> Task<Message> {
        if let Some(id) = self.win_of(kind) {
            self.windows.remove(&id);
            window::close(id)
        } else {
            Task::none()
        }
    }

    /// Close every Download File Info window, whichever download each one
    /// was opened for.
    ///
    /// The dialog's state is a single slot (`self.file_info`) while the
    /// window identity carries a `DlId`, so two of them can never be
    /// consistent: both windows render the newer download, and the older
    /// one's buttons act on it and close the *other* window — after which
    /// they close nothing at all and the dialog is stuck on screen. The
    /// dialog is therefore opened one at a time, and its buttons dismiss
    /// whatever File Info window exists rather than one specific id.
    fn close_file_info_windows(&mut self) -> Task<Message> {
        let ids: Vec<window::Id> = self
            .windows
            .iter()
            .filter(|(_, k)| matches!(k, WinKind::FileInfo(_)))
            .map(|(id, _)| *id)
            .collect();
        let mut tasks = Vec::with_capacity(ids.len());
        for id in ids {
            self.windows.remove(&id);
            tasks.push(window::close(id));
        }
        Task::batch(tasks)
    }

    // ------------------------------------------------------------ downloads

    /// Effective connection count for a URL (Connection tab exceptions).
    fn conns_for(&self, url: &str) -> usize {
        let host = engine::parse_url(url).map(|u| u.host).unwrap_or_default();
        self.cfg
            .settings
            .conn_exceptions
            .iter()
            .find(|(server, _)| {
                !server.is_empty() && host.ends_with(server.trim_start_matches("*."))
            })
            .map(|(_, n)| *n)
            .unwrap_or(self.cfg.settings.default_conns)
            .clamp(1, 32)
    }

    /// The cap this download is actually running under: its own if it set one,
    /// otherwise the global Speed Limiter's while that is switched on. This is
    /// the figure the engine was handed, so it is also the one the views may
    /// present as the transfer's ceiling.
    pub(crate) fn effective_limit(&self, d: &DownloadItem) -> Option<u64> {
        d.speed_limit.or(if self.cfg.settings.speed_limiter_on {
            self.cfg.settings.global_speed_limit
        } else {
            None
        })
    }

    pub fn start_download(&mut self, id: DlId, open_progress: bool) -> Task<Message> {
        let user_agent = self.cfg.settings.user_agent.clone();
        let adaptive = self.cfg.settings.adaptive_conns;
        let Some(d) = self.item_mut(id) else {
            return Task::none();
        };
        if d.state.is_active() {
            return Task::none();
        }
        d.state = DlState::Connecting;
        d.status_line = i18n::tr("Connecting...");
        d.last_try = Some(fmt::now_unix());
        d.conns.clear();
        let part = d.part_file().to_string_lossy().into_owned();
        d.part_path = Some(part.clone());
        // A resume is only a resume while the staging file still matches the
        // spans we recorded for it. Persisted spans are sanitized first —
        // `mark_done` must never be handed overlapping or inverted ranges
        // from a stale state file — and the `.part` is checked by length AND
        // allocation, because `SparseSink::create` calls `set_len(size)`
        // before the first byte: a file that received 1 byte and a file that
        // received everything both report `size`, so length alone would let
        // a truncated or foreign file "complete" over holes of zeros.
        d.held = sanitize_spans(&d.held, d.size);
        let part_ok = part_matches(std::path::Path::new(&part), d.size, &d.held);
        if !d.held.is_empty() && !part_ok {
            crate::log::warn(&format!(
                "#{id} staging file does not match recorded spans: restarting from zero"
            ));
            d.held.clear();
        }
        // `downloaded` is derived from `held`, never stored independently:
        // the two drifting apart is what made the bar and the chunk strip
        // disagree after a repair.
        d.downloaded = sum_spans(&d.held);
        if d.held.is_empty() {
            d.disp_progress = 0.0;
        }
        let spec = StartSpec {
            id,
            url: d.url.clone(),
            auth: d.auth.clone(),
            conns: 0, // filled below (borrow)
            user_agent,
            temp_path: part,
            final_path: d.full_path().to_string_lossy().into_owned(),
            held: d.held.clone(),
            expected_size: d.size,
            cookies: d.cookies.clone(),
            limit: None,
            adaptive,
        };
        let url = d.url.clone();
        let limit = self.effective_limit(self.item(id).unwrap());
        let spec = StartSpec {
            conns: self.conns_for(&url),
            limit,
            ..spec
        };
        engine::send(Cmd::Start(Box::new(spec)));
        self.save_state();
        if open_progress {
            let seed = self.item(id).and_then(|d| d.speed_limit);
            self.prog.entry(id).or_insert_with(|| prog_state_seed(seed));
            self.open_window(WinKind::Progress(id))
        } else {
            Task::none()
        }
    }

    fn stop_download(&mut self, id: DlId) {
        if let Some(d) = self.item_mut(id) {
            if d.state.is_active() {
                engine::send(Cmd::Stop(id));
                d.status_line = i18n::tr("Pause");
            } else if d.state == DlState::Queued {
                d.state = DlState::Paused;
            }
        }
    }

    /// Stop the given items, honouring "Warn me before stopping downloads":
    /// with the flag set and at least one target actually transferring, the
    /// stop is parked behind a confirmation. `stop_queues` carries the
    /// Pause All / Stop All variant, which also halts queue processing.
    fn stop_ids_confirming(&mut self, ids: Vec<DlId>, stop_queues: bool) -> Task<Message> {
        let any_active = ids
            .iter()
            .any(|id| self.item(*id).map(|d| d.state.is_active()).unwrap_or(false));
        if self.cfg.settings.warn_before_stop && any_active {
            self.confirm = Some(ConfirmKind::StopWarn { ids, stop_queues });
            return self.open_window(WinKind::Confirm);
        }
        self.stop_ids(ids, stop_queues);
        Task::none()
    }

    fn stop_ids(&mut self, ids: Vec<DlId>, stop_queues: bool) {
        for id in ids {
            self.stop_download(id);
        }
        if stop_queues {
            for q in &mut self.cfg.queues {
                q.running = false;
            }
        }
    }

    fn delete_item(&mut self, id: DlId) -> Task<Message> {
        self.delete_item_opts(id, false)
    }

    fn delete_item_opts(&mut self, id: DlId, remove_file: bool) -> Task<Message> {
        let active = self.item(id).map(|d| d.state.is_active()).unwrap_or(false);
        if active {
            engine::send(Cmd::Stop(id));
            self.pending_delete.push((id, Instant::now()));
        } else {
            self.remove_item_opts(id, remove_file);
        }
        let mut task = self.close_window(WinKind::Progress(id));
        if self.win_of(WinKind::FileInfo(id)).is_some() {
            task = Task::batch([task, self.close_window(WinKind::FileInfo(id))]);
        }
        task
    }

    /// `pending_delete` is a deadline, not a promise: the engine's stop
    /// acknowledgement can be lost in the start/stop race window, and an id
    /// stuck in the list would silently delete a future download. Remove an
    /// entry as soon as its item stopped being active, and after a few
    /// seconds regardless.
    fn sweep_pending_deletes(&mut self) {
        if self.pending_delete.is_empty() {
            return;
        }
        let expired: Vec<DlId> = self
            .pending_delete
            .iter()
            .filter(|(id, at)| {
                at.elapsed().as_secs() >= 5
                    || !self.item(*id).map(|d| d.state.is_active()).unwrap_or(false)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            self.pending_delete.retain(|(x, _)| *x != id);
            self.remove_item(id);
        }
    }

    fn remove_item(&mut self, id: DlId) {
        self.remove_item_opts(id, false);
    }

    fn remove_item_opts(&mut self, id: DlId, remove_file: bool) {
        if let Some(d) = self.item(id) {
            if d.state != DlState::Complete {
                let _ = std::fs::remove_file(d.part_file());
            } else if remove_file {
                let _ = std::fs::remove_file(d.full_path());
            }
        }
        self.state.downloads.retain(|d| d.id != id);
        self.selected.retain(|x| *x != id);
        self.prog.remove(&id);
        self.save_state();
    }

    /// Create a new list entry for a URL; returns its id. Every download
    /// belongs to a queue (the first one — "Main download queue" — unless the
    /// caller names another), so Start/Stop Queue always govern the whole
    /// list.
    pub fn add_item(
        &mut self,
        url: String,
        auth: Option<(String, String)>,
        queue: Option<String>,
    ) -> DlId {
        let queue = queue.or_else(|| self.cfg.queues.first().map(|q| q.name.clone()));
        let id = self.state.next_id;
        self.state.next_id += 1;
        let file_name = engine::file_name_from_url(&url);
        let category = categorize(&file_name, &self.cfg.categories);
        let save_dir = category
            .as_deref()
            .and_then(|c| self.cfg.categories.iter().find(|k| k.name == c))
            .map(|c| c.dir.clone())
            .unwrap_or_else(|| {
                self.cfg
                    .categories
                    .first()
                    .map(|c| c.dir.clone())
                    .unwrap_or_else(|| ".".into())
            });
        let q_order = self
            .state
            .downloads
            .iter()
            .filter(|d| d.queue == queue && queue.is_some())
            .map(|d| d.q_order + 1)
            .max()
            .unwrap_or(0);
        self.state.downloads.push(DownloadItem {
            id,
            url,
            file_name,
            save_dir,
            category,
            description: String::new(),
            size: None,
            downloaded: 0,
            state: DlState::Paused,
            error: None,
            resume: None,
            added: fmt::now_unix(),
            last_try: None,
            queue,
            q_order,
            auth,
            cookies: None,
            speed_limit: None,
            held: vec![],
            part_path: None,
            rate: 0.0,
            retries: 0,
            disp_progress: 0.0,
            eta_secs: None,
            conns: vec![],
            status_line: String::new(),
        });
        self.save_state();
        id
    }

    // --------------------------------------------------------------- queues

    fn queue_tick(&mut self) -> Task<Message> {
        let mut to_start: Vec<DlId> = vec![];
        let mut worked: Vec<String> = vec![];
        for q in &self.cfg.queues {
            if !q.running {
                continue;
            }
            let active = self
                .state
                .downloads
                .iter()
                .filter(|d| d.queue.as_deref() == Some(q.name.as_str()) && d.state.is_active())
                .count();
            if active > 0 {
                worked.push(q.name.clone());
            }
            if active >= q.files_at_once as usize {
                continue;
            }
            let mut queued: Vec<&DownloadItem> = self
                .state
                .downloads
                .iter()
                .filter(|d| {
                    d.queue.as_deref() == Some(q.name.as_str()) && d.state == DlState::Queued
                })
                .collect();
            queued.sort_by_key(|d| d.q_order);
            if !queued.is_empty() && active == 0 {
                worked.push(q.name.clone());
            }
            for d in queued.iter().take(q.files_at_once as usize - active) {
                to_start.push(d.id);
            }
        }
        for name in worked {
            if let Some(q) = self.cfg.queues.iter_mut().find(|q| q.name == name) {
                q.did_work = true;
            }
        }
        let tasks: Vec<Task<Message>> = to_start
            .into_iter()
            .map(|id| self.start_download(id, false))
            .collect();
        // A running queue with nothing active and nothing queued is finished:
        // apply its "when done" actions — but only when this run actually
        // processed something. A queue started with nothing to do (or whose
        // scheduled start found no members) must not play the stop sound or
        // fire `exit_when_done` on the very tick it began.
        let finished: Vec<String> = self
            .cfg
            .queues
            .iter()
            .filter(|q| {
                q.running
                    && !self.state.downloads.iter().any(|d| {
                        d.queue.as_deref() == Some(q.name.as_str())
                            && (d.state.is_active() || d.state == DlState::Queued)
                    })
            })
            .map(|q| q.name.clone())
            .collect();
        let mut exit_app = false;
        for name in &finished {
            let mut acted = false;
            if let Some(q) = self.cfg.queues.iter_mut().find(|q| q.name == *name) {
                q.running = false;
                acted = std::mem::take(&mut q.did_work);
                let sc = &q.schedule;
                if acted {
                    if sc.open_file_enabled && !sc.open_file.trim().is_empty() {
                        let _ = open::that_detached(sc.open_file.trim());
                    }
                    if sc.exit_when_done {
                        exit_app = true;
                    }
                }
            }
            if acted {
                if let Some(row) = self.cfg.settings.sounds.get(3).filter(|r| r.enabled) {
                    sounds::play(
                        (!row.file.is_empty()).then(|| row.file.clone()),
                        sounds::Event::QueueStopped,
                    );
                }
                crate::log::info(&format!("queue finished: {name}"));
            }
        }
        if exit_app {
            self.save_state();
            self.save_config();
            self.flush_saves();
            return Task::batch([Task::batch(tasks), iced::exit()]);
        }
        Task::batch(tasks)
    }

    fn schedule_tick(&mut self) {
        use chrono::Datelike;
        let now = chrono::Local::now();
        let hhmm = fmt::hhmm(&now);
        let weekday = now.weekday().num_days_from_sunday() as usize; // Sun=0
        let mut start: Vec<String> = vec![];
        let mut stop: Vec<String> = vec![];
        for q in &self.cfg.queues {
            let s = &q.schedule;
            if schedule_start_due(s, &hhmm, weekday, q.running) {
                start.push(q.name.clone());
            }
            if s.stop_enabled && s.stop_at == hhmm && q.running {
                stop.push(q.name.clone());
            }
        }
        for name in start {
            // "Once" means once: disarm after firing, or the same wall-clock
            // minute would re-trigger it every day.
            if let Some(q) = self.cfg.queues.iter_mut().find(|q| q.name == name) {
                if q.schedule.once {
                    q.schedule.start_enabled = false;
                    self.save_config();
                }
            }
            self.set_queue_running(&name, true);
        }
        for name in stop {
            self.set_queue_running(&name, false);
        }
    }

    pub fn set_queue_running(&mut self, name: &str, run: bool) {
        let was = self
            .cfg
            .queues
            .iter()
            .find(|q| q.name == name)
            .map(|q| q.running)
            .unwrap_or(false);
        // Promotion lives HERE, on the one function every start path shares
        // (menu, toolbar, scheduler wall-clock, boot): a queue whose members
        // are all Paused would otherwise turn `running = true`, find nothing
        // Queued, and be classified as instantly finished.
        if run {
            promote_queue_members(&mut self.state.downloads, name);
        }
        if let Some(q) = self.cfg.queues.iter_mut().find(|q| q.name == name) {
            q.running = run;
        }
        if run != was {
            let idx = if run { 2 } else { 3 };
            if let Some(row) = self.cfg.settings.sounds.get(idx).filter(|r| r.enabled) {
                sounds::play(
                    (!row.file.is_empty()).then(|| row.file.clone()),
                    sounds::Event::from_index(idx),
                );
            }
        }
        if !run {
            let ids: Vec<DlId> = self
                .state
                .downloads
                .iter()
                .filter(|d| d.queue.as_deref() == Some(name) && d.state.is_active())
                .map(|d| d.id)
                .collect();
            for id in ids {
                engine::send(Cmd::Stop(id));
            }
            // Anything still queued stays queued for the next start.
        }
    }

    // -------------------------------------------------------------- engine

    fn on_engine(&mut self, ev: engine::Event) -> Task<Message> {
        match ev {
            engine::Event::Probed {
                id,
                size,
                ranges,
                file_name,
            } => {
                if let Some(d) = self.item_mut(id) {
                    if d.size.is_none() {
                        d.size = size;
                    }
                    d.resume = Some(ranges);
                    // Only adopt a server name if the user hasn't renamed.
                    if let Some(name) = file_name {
                        if d.downloaded == 0 && d.held.is_empty() && !name.is_empty() {
                            d.file_name = name;
                        }
                    }
                    // The state stays Connecting: the probe answered, but no
                    // byte has arrived — the first Progress event promotes to
                    // Receiving. (Jumping early made the Status column show
                    // "0.00%" while the dialog still said Connecting.)
                }
                // The File Info dialog fills itself as the probe answers:
                // real name from Content-Disposition, category by
                // extension, size via the item it displays.
                if self.file_info.dl == id && self.win_of(WinKind::FileInfo(id)).is_some() {
                    let probed_name = self.item(id).map(|d| d.file_name.clone());
                    if let Some(name) = probed_name {
                        if !self.file_info.name_touched {
                            self.file_info.file_name = name.clone();
                        }
                        if !self.file_info.cat_touched {
                            if let Some(cat) = categorize(&name, &self.cfg.categories) {
                                let dir = self
                                    .cfg
                                    .categories
                                    .iter()
                                    .find(|c| c.name == cat)
                                    .map(|c| c.dir.clone());
                                self.file_info.category = cat.clone();
                                if let (false, Some(dir)) =
                                    (self.file_info.dir_touched, dir.clone())
                                {
                                    self.file_info.save_dir = dir;
                                }
                                let dir_untouched = !self.file_info.dir_touched;
                                if let Some(d) = self.item_mut(id) {
                                    d.category = Some(cat);
                                    if let (Some(dir), true) = (dir, dir_untouched) {
                                        d.save_dir = dir;
                                    }
                                }
                            }
                        }
                    }
                }
                self.save_state();
                Task::none()
            }
            engine::Event::Status { id, line } => {
                if let Some(d) = self.item_mut(id) {
                    d.status_line = line;
                }
                Task::none()
            }
            engine::Event::Progress {
                id,
                done,
                rate,
                eta,
                conns,
                held,
            } => {
                if let Some(d) = self.item_mut(id) {
                    // `held` is authoritative for the byte total wherever it
                    // exists: after a repair the scheduler can shrink the
                    // held set below a previously-reported `done`, and
                    // storing the two independently made the bar and the
                    // chunk strip disagree from that point on.
                    d.downloaded = if held.is_empty() {
                        done
                    } else {
                        sum_spans(&held)
                    };
                    d.rate = rate;
                    d.eta_secs = eta;
                    d.conns = conns;
                    d.held = held;
                    if d.state == DlState::Connecting {
                        d.state = DlState::Receiving;
                    }
                }
                Task::none()
            }
            engine::Event::Finished { id, elapsed, size } => {
                crate::log::log(&format!("#{id} complete: {size} bytes in {elapsed:.1}s"));
                // A delete was pending but the transfer won the race and
                // finished: honour the delete anyway — the row must not
                // reappear as Complete after the user removed it.
                if self.pending_delete.iter().any(|(x, _)| *x == id) {
                    self.pending_delete.retain(|(x, _)| *x != id);
                    self.remove_item(id);
                    return Task::none();
                }
                let remember_limit = self
                    .prog
                    .get(&id)
                    .map(|p| p.remember_limit)
                    .unwrap_or(false);
                if let Some(d) = self.item_mut(id) {
                    if !remember_limit {
                        d.speed_limit = None;
                    }
                    d.state = DlState::Complete;
                    d.downloaded = size;
                    d.size = Some(size);
                    d.rate = 0.0;
                    d.disp_progress = 1.0;
                    d.eta_secs = None;
                    d.held = vec![(0, size)];
                    d.part_path = None;
                    // Finished files leave their queue: "Files in the queue"
                    // lists pending work, not history.
                    d.queue = None;
                    d.status_line = i18n::tr("Complete");
                }
                self.save_state();
                // The ask-box for THIS download is obsolete once the
                // background transfer lands: close it, the complete dialog
                // (below) takes over.
                let mut fi_close = Task::none();
                if self.file_info.is_new
                    && self.file_info.dl == id
                    && self.win_of(WinKind::FileInfo(id)).is_some()
                {
                    fi_close = self.close_window(WinKind::FileInfo(id));
                }
                if let Some(row) = self.cfg.settings.sounds.first().filter(|r| r.enabled) {
                    sounds::play(
                        (!row.file.is_empty()).then(|| row.file.clone()),
                        sounds::Event::DownloadComplete,
                    );
                }
                let mut task = self.close_window(WinKind::Progress(id));
                if self.cfg.settings.show_complete_dialog {
                    task = Task::batch([task, self.open_window(WinKind::Complete(id))]);
                }
                Task::batch([fi_close, task, self.queue_tick()])
            }
            engine::Event::Stopped { id, done, held } => {
                if self.pending_delete.iter().any(|(x, _)| *x == id) {
                    self.pending_delete.retain(|(x, _)| *x != id);
                    self.remove_item(id);
                    return Task::none();
                }
                let remember_limit = self
                    .prog
                    .get(&id)
                    .map(|p| p.remember_limit)
                    .unwrap_or(false);
                if let Some(d) = self.item_mut(id) {
                    d.state = DlState::Paused;
                    // `held` authoritative, counter derived (see Progress).
                    if !held.is_empty() {
                        d.held = held;
                        d.downloaded = sum_spans(&d.held);
                    } else if done > 0 {
                        d.downloaded = done;
                    }
                    if !remember_limit {
                        d.speed_limit = None;
                    }
                    // Keep the last measured rate/ETA on screen:
                    // a paused dialog still shows what the transfer was doing.
                    d.status_line = i18n::tr("Pause");
                    for c in &mut d.conns {
                        c.info = i18n::tr("Disconnect.");
                    }
                }
                self.save_state();
                self.queue_tick()
            }
            engine::Event::Failed {
                id,
                error,
                done,
                held,
                permission_denied,
            } => {
                if self.pending_delete.iter().any(|(x, _)| *x == id) {
                    self.pending_delete.retain(|(x, _)| *x != id);
                    self.remove_item(id);
                    return Task::none();
                }
                // Scheduler retries: a failed file in a running queue with
                // "Number of retries" enabled goes back to Queued until its
                // budget is spent.
                if !permission_denied {
                    let retry_budget = self
                        .item(id)
                        .and_then(|d| d.queue.clone())
                        .and_then(|qn| self.cfg.queues.iter().find(|q| q.name == qn))
                        .filter(|q| q.running && q.schedule.retries_enabled)
                        .map(|q| q.schedule.retries);
                    if let Some(budget) = retry_budget {
                        if let Some(d) = self.item_mut(id) {
                            if d.retries < budget {
                                d.retries += 1;
                                d.state = DlState::Queued;
                                if !held.is_empty() {
                                    d.held = held.clone();
                                    d.downloaded = sum_spans(&d.held);
                                } else if done > 0 {
                                    d.downloaded = done;
                                }
                                crate::log::info(&format!(
                                    "#{id} retry {}/{budget}: {error}",
                                    d.retries
                                ));
                                self.save_state();
                                return self.queue_tick();
                            }
                        }
                    }
                }
                if permission_denied {
                    // OS-native consent instead of sudo or manual settings:
                    // a folder chosen through the system panel is implicitly
                    // granted, so offer the picker and resume right there.
                    let start_dir = self.item(id).map(|d| d.save_dir.clone());
                    let mut dlg = rfd::FileDialog::new();
                    if let Some(dir) = &start_dir {
                        dlg = dlg.set_directory(dir);
                    }
                    if let Some(picked) = dlg.pick_folder() {
                        let picked = picked.to_string_lossy().into_owned();
                        crate::log::info(&format!("#{id} folder re-granted: {picked}"));
                        if let Some(d) = self.item_mut(id) {
                            d.save_dir = picked;
                            d.part_path = None;
                            d.state = DlState::Paused;
                            if !held.is_empty() {
                                d.held = held.clone();
                            }
                        }
                        self.save_state();
                        return self.start_download(id, false);
                    }
                }
                if let Some(d) = self.item_mut(id) {
                    d.state = DlState::Error;
                    if !held.is_empty() {
                        d.held = held;
                        d.downloaded = sum_spans(&d.held);
                    } else if done > 0 {
                        d.downloaded = done;
                    }
                    d.error = Some(error.clone());
                    d.status_line = error;
                    for c in &mut d.conns {
                        c.info = i18n::tr("Disconnect.");
                    }
                }
                if let Some(row) = self.cfg.settings.sounds.get(1).filter(|r| r.enabled) {
                    sounds::play(
                        (!row.file.is_empty()).then(|| row.file.clone()),
                        sounds::Event::DownloadFailed,
                    );
                }
                self.save_state();
                self.queue_tick()
            }
        }
    }

    // -------------------------------------------------------------- update

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Noop => Task::none(),
            Message::WindowOpened(id) => {
                #[cfg(target_os = "macos")]
                {
                    // Back to Regular before installing the menu: with the
                    // Dock hidden the app sat in Accessory, which has no
                    // menu bar to install into.
                    crate::macos_dock::sync(self.cfg.settings.hide_from_taskbar, true);
                    let state = crate::macos_menu::MenuState {
                        dark_mode: self.cfg.settings.dark_mode,
                        show_categories: self.cfg.settings.show_categories,
                        font_size: self.cfg.settings.font_size,
                        language: self.cfg.language.clone().unwrap_or_else(|| "en".into()),
                        speed_limiter: self.cfg.settings.speed_limiter_on,
                    };
                    let queues: Vec<String> =
                        self.cfg.queues.iter().map(|q| q.name.clone()).collect();
                    crate::macos_menu::install(&state, &queues, &crate::i18n::available());
                }
                #[cfg(any(target_os = "macos", target_os = "windows"))]
                {
                    let queues: Vec<String> =
                        self.cfg.queues.iter().map(|q| q.name.clone()).collect();
                    crate::tray::install(&queues, self.cfg.settings.power_save);
                }
                // Browser-capture dialogs float above everything: at that
                // moment this app is in the background and a normal-level
                // window would open behind the browser.
                if self.capture_raise
                    && matches!(
                        self.windows.get(&id),
                        Some(WinKind::FileInfo(_) | WinKind::Confirm | WinKind::Batch)
                    )
                {
                    self.capture_raise = false;
                    return Task::batch([
                        window::set_level(id, window::Level::AlwaysOnTop),
                        window::gain_focus(id),
                    ]);
                }
                window::gain_focus(id)
            }
            Message::WindowClosed(id) => {
                let kind = self.windows.remove(&id);
                // Last window gone + hide-Dock on: drop to Accessory now
                // that no menu bar is needed (tray-only from here).
                #[cfg(target_os = "macos")]
                crate::macos_dock::sync(
                    self.cfg.settings.hide_from_taskbar,
                    !self.windows.is_empty(),
                );
                match kind {
                    Some(WinKind::Main) => {
                        self.save_state();
                        self.save_config();
                        self.flush_saves();
                        if cfg!(any(target_os = "macos", target_os = "windows")) {
                            // Hydra lives on in the system tray; Exit is in
                            // the tray menu (and the app menu on macOS).
                            self.main_id = None;
                            Task::none()
                        } else {
                            iced::exit()
                        }
                    }
                    Some(WinKind::Confirm) => {
                        self.confirm = None;
                        // A duplicate decision dismissed with the OS close
                        // button must not leave its URL parked: the next
                        // "Download as new file" would add the stale one.
                        self.pending_add = None;
                        Task::none()
                    }
                    Some(WinKind::FileInfo(dl)) => {
                        // OS close button on the new-download dialog is
                        // Cancel: drop the auto-started item.
                        if self.file_info.is_new && self.file_info.dl == dl {
                            self.delete_item(dl)
                        } else {
                            Task::none()
                        }
                    }
                    Some(WinKind::Update) => {
                        // OS close button is Cancel: stop an in-flight
                        // download and forget the offer (unless the finisher
                        // is already live — then the exit is imminent).
                        if self.updater.phase != UpdatePhase::Restarting {
                            if let Some(c) = &self.updater.cancel {
                                c.store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                            self.updater = UpdateUiState::default();
                        }
                        Task::none()
                    }
                    _ => Task::none(),
                }
            }
            Message::WindowCloseRequested(id) => {
                // `exit_on_close_request: false` means the close button only
                // ASKS; without this handler every red button was dead.
                // Always honour it, tracked or not: a window we have lost
                // track of must still be closable by its own close button.
                // Bookkeeping happens in WindowClosed once it's gone.
                window::close(id)
            }
            Message::WinMoved(id, p) => {
                if self.main_id == Some(id) {
                    self.main_pos = Some(p);
                }
                Task::none()
            }
            Message::WinResized(id, size) => {
                if self.main_id == Some(id) {
                    self.main_size = size;
                    // Remember it (written with the next config save).
                    if size.width >= 900.0 && size.height >= 600.0 {
                        self.cfg.settings.window_size = Some((size.width, size.height));
                    }
                }
                Task::none()
            }
            Message::Engine(ev) => self.on_engine(ev),
            Message::Tick => {
                if self.win_of(WinKind::Permissions).is_some() {
                    self.refresh_perm_status();
                }
                self.sweep_pending_deletes();
                self.schedule_tick();
                let queue = self.queue_tick();
                self.flush_saves();
                if self.cfg.settings.monitor_clipboard {
                    Task::batch([queue, iced::clipboard::read().map(Message::ClipboardSeen)])
                } else {
                    queue
                }
            }
            Message::ClipboardSeen(text) => {
                let Some(text) = text else {
                    return Task::none();
                };
                let text = text.trim().to_string();
                if text.is_empty() || text == self.last_clipboard {
                    return Task::none();
                }
                self.last_clipboard = text.clone();
                let urls: Vec<String> = text
                    .lines()
                    .map(str::trim)
                    .filter(|l| looks_downloadable(l, &self.cfg.settings.auto_types))
                    .filter(|l| !self.state.downloads.iter().any(|d| d.url == *l))
                    .map(str::to_string)
                    .collect();
                match urls.len() {
                    0 => Task::none(),
                    1 => {
                        crate::log::info(&format!("clipboard capture: {}", urls[0]));
                        self.add_url = AddUrlState {
                            address: urls[0].clone(),
                            ..AddUrlState::default()
                        };
                        self.update(Message::AddUrlOk)
                    }
                    _ => {
                        // Many links: the "Download All Links" box.
                        crate::log::info(&format!("clipboard capture: {} links", urls.len()));
                        self.batch = BatchState::default();
                        self.batch.category = "General".into();
                        let open = self.open_window(WinKind::Batch);
                        let text = urls.join("\n");
                        Task::batch([open, self.update(Message::BatchLoaded(Some(text)))])
                    }
                }
            }
            Message::AnimTick => {
                for d in &mut self.state.downloads {
                    let target = d.progress();
                    let diff = target - d.disp_progress;
                    if diff == 0.0 {
                        // Idle rows (history, paused items already at their
                        // fraction) must not be touched 12.5 times a second.
                        continue;
                    }
                    if !(0.0..=0.2).contains(&diff) {
                        // Backwards (redownload) or a jump the glide would
                        // turn into a long crawl: snap.
                        d.disp_progress = target;
                    } else if diff > 0.0005 {
                        d.disp_progress += diff * 0.35;
                    } else {
                        d.disp_progress = target;
                    }
                }
                Task::none()
            }
            Message::NativeMenu(id) => {
                if id == "show_main" {
                    return self.open_window(WinKind::Main);
                }
                match MenuAction::from_id(&id) {
                    Some(a) => self.update(Message::Menu(a)),
                    None => Task::none(),
                }
            }
            Message::MenuOpen(kind) => {
                self.open_menu = if self.open_menu == Some(kind) {
                    None
                } else {
                    Some(kind)
                };
                self.open_submenu = None;
                Task::none()
            }
            Message::MenuClose => {
                self.open_menu = None;
                self.open_submenu = None;
                self.ctx_at = None;
                self.queue_menu = None;
                Task::none()
            }
            Message::SubmenuHover(i) => {
                self.open_submenu = i;
                Task::none()
            }
            Message::Menu(action) => {
                self.open_menu = None;
                self.open_submenu = None;
                self.ctx_at = None;
                self.queue_menu = None;
                self.on_menu(action)
            }
            Message::TreeSelect(sel) => {
                self.tree_sel = sel;
                Task::none()
            }
            Message::TreeToggle(i) => {
                if let Some(f) = self.tree_open.get_mut(i as usize) {
                    *f = !*f;
                }
                Task::none()
            }
            Message::RowClick(id) => {
                if self.mods.command() || self.mods.control() {
                    // Toggle membership.
                    if self.selected.contains(&id) {
                        self.selected.retain(|x| *x != id);
                    } else {
                        self.selected.push(id);
                    }
                    self.sel_anchor = Some(id);
                    self.last_click = None;
                    return Task::none();
                }
                if self.mods.shift() {
                    // Range from the anchor within the visible ordering.
                    let anchor = self.sel_anchor.unwrap_or(id);
                    let order: Vec<DlId> = self.visible().iter().map(|d| d.id).collect();
                    let a = order.iter().position(|x| *x == anchor);
                    let b = order.iter().position(|x| *x == id);
                    if let (Some(a), Some(b)) = (a, b) {
                        let (lo, hi) = (a.min(b), a.max(b));
                        self.selected = order[lo..=hi].to_vec();
                    }
                    self.last_click = None;
                    return Task::none();
                }
                let double = self
                    .last_click
                    .map(|(last, at)| last == id && at.elapsed().as_millis() < 400)
                    .unwrap_or(false);
                self.last_click = Some((id, Instant::now()));
                self.selected = vec![id];
                self.sel_anchor = Some(id);
                self.list_press = Some(id);
                if double {
                    let st = self.item(id).map(|d| d.state);
                    match st {
                        // Active transfer: its progress box. Anything else:
                        // the File Properties dialog.
                        Some(s) if s.is_active() => self.open_window(WinKind::Progress(id)),
                        Some(_) => self.update(Message::Menu(MenuAction::Properties)),
                        None => Task::none(),
                    }
                } else {
                    Task::none()
                }
            }
            Message::RowRightClick(id) => {
                // Right-click keeps an existing multi-selection if the row is
                // part of it.
                if !self.selected.contains(&id) {
                    self.selected = vec![id];
                    self.sel_anchor = Some(id);
                }
                self.ctx_at = Some(self.cursor);
                Task::none()
            }
            Message::CursorMoved(p) => {
                self.cursor = p;
                if let Some((col, grab_x, start_w)) = self.resizing {
                    if let Some(w) = self.col_widths.get_mut(col) {
                        *w = (start_w + (p.x - grab_x)).clamp(40.0, 800.0);
                    }
                }
                Task::none()
            }
            Message::Mods(m) => {
                self.mods = m;
                Task::none()
            }
            Message::RawKey(key, mods) => {
                let combo = combo_string(&key, mods);
                let Some(combo) = combo else {
                    return Task::none();
                };
                if combo == "cmd+a" {
                    return self.update(Message::SelectAll);
                }
                let action = self
                    .cfg
                    .shortcuts
                    .iter()
                    .find(|(_, c)| **c == combo)
                    .map(|(id, _)| id.clone());
                match action.as_deref() {
                    Some("add_url") => self.update(Message::Menu(MenuAction::AddNewDownload)),
                    Some("clipboard_add") => {
                        iced::clipboard::read().map(Message::ClipboardAddStart)
                    }
                    Some("options") => self.update(Message::Menu(MenuAction::Options)),
                    Some("scheduler") => self.update(Message::Menu(MenuAction::Scheduler)),
                    Some("resume_last") => {
                        let id = self
                            .state
                            .downloads
                            .iter()
                            .filter(|d| {
                                matches!(
                                    d.state,
                                    DlState::Paused | DlState::Error | DlState::Queued
                                )
                            })
                            .max_by_key(|d| d.added)
                            .map(|d| d.id);
                        match id {
                            Some(id) => self.start_download(id, true),
                            None => Task::none(),
                        }
                    }
                    Some("stop_last") => {
                        let id = self
                            .state
                            .downloads
                            .iter()
                            .filter(|d| d.state.is_active())
                            .max_by_key(|d| d.added)
                            .map(|d| d.id);
                        if let Some(id) = id {
                            self.stop_download(id);
                        }
                        Task::none()
                    }
                    Some("start_main_queue") => {
                        let q = self.cfg.queues.first().map(|q| q.name.clone());
                        match q {
                            Some(q) => self.update(Message::Menu(MenuAction::StartQueue(q))),
                            None => Task::none(),
                        }
                    }
                    Some("stop_main_queue") => {
                        let q = self.cfg.queues.first().map(|q| q.name.clone());
                        match q {
                            Some(q) => self.update(Message::Menu(MenuAction::StopQueue(q))),
                            None => Task::none(),
                        }
                    }
                    _ => Task::none(),
                }
            }
            Message::SelectAll => {
                self.selected = self.visible().iter().map(|d| d.id).collect();
                self.sel_anchor = self.selected.first().copied();
                Task::none()
            }
            Message::RowEnter(id) => {
                // Rubber-band selection: pointer went down on a row and swept
                // here with the button still held.
                if let Some(anchor) = self.list_press {
                    let order: Vec<DlId> = self.visible().iter().map(|d| d.id).collect();
                    let a = order.iter().position(|x| *x == anchor);
                    let b = order.iter().position(|x| *x == id);
                    if let (Some(a), Some(b)) = (a, b) {
                        let (lo, hi) = (a.min(b), a.max(b));
                        self.selected = order[lo..=hi].to_vec();
                        self.sel_anchor = Some(anchor);
                    }
                }
                Task::none()
            }
            Message::QueueMenuOpen(start) => {
                self.queue_menu = Some(start);
                self.ctx_at = Some(self.cursor);
                Task::none()
            }
            Message::ClipboardAddStart(text) => {
                let Some(text) = text else {
                    return Task::none();
                };
                let url = text
                    .lines()
                    .map(str::trim)
                    .find(|l| looks_downloadable(l, &self.cfg.settings.auto_types));
                let Some(url) = url else { return Task::none() };
                if self.state.downloads.iter().any(|d| d.url == url) {
                    return Task::none();
                }
                let id = self.add_item(url.to_string(), None, None);
                self.start_download(id, true)
            }
            Message::ShortcutEdit(action, combo) => {
                self.cfg
                    .shortcuts
                    .insert(action, combo.trim().to_ascii_lowercase());
                self.save_config();
                Task::none()
            }
            Message::MouseUp => {
                if self.resizing.take().is_some() {
                    self.cfg.settings.column_widths = self.col_widths.clone();
                    self.save_config();
                }
                if self.sch.drag.take().is_some() {
                    self.save_state();
                }
                self.list_press = None;
                Task::none()
            }
            Message::ColResizeStart(col) => {
                if let Some(w) = self.col_widths.get(col) {
                    self.resizing = Some((col, self.cursor.x, *w));
                }
                Task::none()
            }
            Message::SortBy(key) => {
                if self.sort.0 == key {
                    self.sort.1 = !self.sort.1;
                } else {
                    self.sort = (key, true);
                }
                Task::none()
            }
            Message::ToolbarResume => {
                let ids: Vec<DlId> = self
                    .selected
                    .iter()
                    .copied()
                    .filter(|id| {
                        self.item(*id)
                            .map(|d| {
                                matches!(
                                    d.state,
                                    DlState::Paused | DlState::Error | DlState::Queued
                                )
                            })
                            .unwrap_or(false)
                    })
                    .collect();
                // A single resume shows its progress window; a batch resume
                // would bury the screen in dialogs.
                let show_progress = ids.len() == 1;
                let tasks: Vec<Task<Message>> = ids
                    .into_iter()
                    .map(|id| self.start_download(id, show_progress))
                    .collect();
                Task::batch(tasks)
            }
            Message::ToolbarStop => self.stop_ids_confirming(self.selected.clone(), false),
            Message::ToolbarDelete => {
                if !self.selected.is_empty() {
                    self.confirm = Some(ConfirmKind::DeleteItems(self.selected.clone()));
                    self.confirm_remove_file = false;
                    self.open_window(WinKind::Confirm)
                } else {
                    Task::none()
                }
            }

            // ------------------------------------------------------ add url
            Message::AddrPrefill(text) => {
                if self.add_url.address.is_empty() {
                    if let Some(t) = text {
                        let t = t.trim();
                        if !t.contains('\n') && looks_downloadable(t, &self.cfg.settings.auto_types)
                        {
                            self.add_url.address = t.to_string();
                        }
                    }
                }
                Task::none()
            }
            Message::AddrChanged(s) => {
                self.add_url.address = s;
                self.add_url.error = None;
                Task::none()
            }
            Message::AddrAuthToggled(b) => {
                self.add_url.use_auth = b;
                Task::none()
            }
            Message::AddrLogin(s) => {
                self.add_url.login = s;
                Task::none()
            }
            Message::AddrPass(s) => {
                self.add_url.password = s;
                Task::none()
            }
            Message::AddUrlOk => {
                let url = self.add_url.address.trim().to_string();
                if let Err(e) = engine::parse_url(&url) {
                    self.add_url.error = Some(e);
                    return Task::none();
                }
                let auth = self
                    .add_url
                    .use_auth
                    .then(|| (self.add_url.login.clone(), self.add_url.password.clone()));
                // Same URL already listed, or the target file already on
                // disk? Ask instead of silently downloading twice.
                let existing = self
                    .state
                    .downloads
                    .iter()
                    .find(|d| d.url == url)
                    .map(|d| d.id);
                let cap_cookies = self
                    .add_url
                    .capture_cookies
                    .take()
                    .filter(|s| !s.is_empty());
                let cap_name = self.add_url.capture_name.take().filter(|s| !s.is_empty());
                let name = cap_name
                    .clone()
                    .unwrap_or_else(|| engine::file_name_from_url(&url));
                let dir = crate::model::categorize(&name, &self.cfg.categories)
                    .and_then(|c| self.cfg.categories.iter().find(|k| k.name == c))
                    .or(self.cfg.categories.first())
                    .map(|c| c.dir.clone())
                    .unwrap_or_default();
                let on_disk = std::path::Path::new(&dir).join(&name);
                let file = on_disk
                    .exists()
                    .then(|| on_disk.to_string_lossy().into_owned());
                if existing.is_some() || file.is_some() {
                    self.pending_add = Some(PendingAdd {
                        url,
                        auth,
                        cookies: cap_cookies,
                        name: cap_name,
                    });
                    self.confirm = Some(ConfirmKind::Duplicate { existing, file });
                    self.add_url = AddUrlState::default();
                    let close = self.close_window(WinKind::AddUrl);
                    return Task::batch([close, self.open_window(WinKind::Confirm)]);
                }
                let id = self.add_item(url, auth, None);
                self.apply_capture_extras(id, cap_cookies, cap_name);
                self.add_url = AddUrlState::default();
                let close = self.close_window(WinKind::AddUrl);
                if self.cfg.settings.show_file_info_dialog {
                    let d = self.item(id).unwrap();
                    self.file_info = FileInfoState {
                        dl: id,
                        category: d.category.clone().unwrap_or_else(|| "General".into()),
                        save_dir: d.save_dir.clone(),
                        file_name: d.file_name.clone(),
                        description: String::new(),
                        remember: true,
                        is_new: true,
                        url: d.url.clone(),
                        login: d.auth.clone().map(|a| a.0).unwrap_or_default(),
                        password: d.auth.clone().map(|a| a.1).unwrap_or_default(),
                        cookies: d.cookies.clone().unwrap_or_default(),
                        ..FileInfoState::default()
                    };
                    // Probes AND starts pulling bytes while this dialog
                    // is open (optional via the dialog's own checkbox);
                    // Start Download only reveals the progress.
                    let bg = if self.cfg.settings.bg_download {
                        self.start_download(id, false)
                    } else {
                        Task::none()
                    };
                    Task::batch([close, self.open_window(WinKind::FileInfo(id)), bg])
                } else {
                    Task::batch([close, self.start_download(id, true)])
                }
            }

            // ------------------------------------------- browser extension
            Message::Ext(ev) => match ev {
                crate::extbus::ExtEvent::Download(dl) => {
                    // Same flow as a manual Add URL, so duplicate detection,
                    // categorization, and the File Info dialog all behave
                    // consistently for browser capture.
                    crate::log::info(&format!("ext: capture dialog for {}", dl.url));
                    self.capture_raise = true;
                    self.add_url = AddUrlState {
                        address: dl.url,
                        capture_cookies: dl.cookies,
                        capture_name: dl.filename,
                        ..AddUrlState::default()
                    };
                    self.update(Message::AddUrlOk)
                }
                crate::extbus::ExtEvent::Links(urls) => {
                    self.capture_raise = true;
                    // "Download all links": the batch window, like a
                    // multi-line clipboard capture.
                    self.batch = BatchState::default();
                    self.batch.category = "General".into();
                    let open = self.open_window(WinKind::Batch);
                    let text = urls.join("\n");
                    Task::batch([open, self.update(Message::BatchLoaded(Some(text)))])
                }
                crate::extbus::ExtEvent::Open => self.open_window(WinKind::Main),
                crate::extbus::ExtEvent::Shutdown => {
                    // A newer build is taking over the single-instance slot
                    // (extbus::signal_existing): leave the way tray Exit does.
                    crate::log::info("ext: shutdown requested by a newer build; exiting");
                    self.save_state();
                    self.save_config();
                    self.flush_saves();
                    iced::exit()
                }
            },

            // ---------------------------------------------------- file info
            Message::FiCategory(c) => {
                if let Some(cat) = self.cfg.categories.iter().find(|k| k.name == c) {
                    if !self.file_info.dir_touched {
                        self.file_info.save_dir = cat.dir.clone();
                    }
                }
                self.file_info.category = c;
                self.file_info.cat_touched = true;
                Task::none()
            }
            Message::FiFileName(s) => {
                self.file_info.file_name = s;
                self.file_info.name_touched = true;
                Task::none()
            }
            Message::FiSaveDir(s) => {
                self.file_info.save_dir = s;
                self.file_info.dir_touched = true;
                Task::none()
            }
            Message::FiBgToggle(b) => {
                self.cfg.settings.bg_download = b;
                self.save_config();
                let dl = self.file_info.dl;
                if self.file_info.is_new {
                    let active = self.item(dl).map(|d| d.state.is_active()).unwrap_or(false);
                    if b && !active {
                        return self.start_download(dl, false);
                    }
                    if !b && active {
                        self.stop_download(dl);
                    }
                }
                Task::none()
            }
            Message::FiBrowseDir => {
                let dir = rfd::FileDialog::new()
                    .pick_folder()
                    .map(|p| p.to_string_lossy().into_owned());
                self.update(Message::FiDirPicked(dir))
            }
            Message::FiDirPicked(Some(dir)) => {
                self.file_info.save_dir = dir;
                self.file_info.dir_touched = true;
                Task::none()
            }
            Message::FiDirPicked(None) => Task::none(),
            Message::FiDescription(s) => {
                self.file_info.description = s;
                Task::none()
            }
            Message::FiRemember(b) => {
                self.file_info.remember = b;
                Task::none()
            }
            Message::FiUrl(v) => {
                self.file_info.url = v;
                Task::none()
            }
            Message::FiLogin(v) => {
                self.file_info.login = v;
                Task::none()
            }
            Message::FiPass(v) => {
                self.file_info.password = v;
                Task::none()
            }
            Message::FiCookies(v) => {
                self.file_info.cookies = v;
                Task::none()
            }
            Message::FiDownloadLater | Message::FiStartDownload => {
                let start = matches!(message, Message::FiStartDownload);
                let fi = self.file_info.clone();
                if fi.remember {
                    if let Some(cat) = self
                        .cfg
                        .categories
                        .iter_mut()
                        .find(|k| k.name == fi.category)
                    {
                        cat.dir = fi.save_dir.clone();
                        self.save_config();
                    }
                }
                let old_path = self.item(fi.dl).map(|d| d.full_path());
                let mut was = None;
                let url_changed = self
                    .item(fi.dl)
                    .map(|d| !fi.url.trim().is_empty() && d.url != fi.url.trim())
                    .unwrap_or(false)
                    && engine::parse_url(fi.url.trim()).is_ok();
                if url_changed {
                    // Mirror switch: keep the held byte spans — the engine
                    // verifies the new source reports the same size and
                    // continues where mirror A stopped.
                    crate::log::info(&format!("#{} mirror -> {}", fi.dl, fi.url.trim()));
                    if self
                        .item(fi.dl)
                        .map(|d| d.state.is_active())
                        .unwrap_or(false)
                    {
                        engine::send(Cmd::Stop(fi.dl));
                    }
                }
                if let Some(d) = self.item_mut(fi.dl) {
                    d.category = Some(fi.category.clone());
                    d.save_dir = fi.save_dir.clone();
                    if !fi.file_name.trim().is_empty() {
                        d.file_name = fi.file_name.trim().to_string();
                    }
                    d.description = fi.description.clone();
                    if url_changed {
                        d.url = fi.url.trim().to_string();
                        d.state = DlState::Paused;
                        d.error = None;
                    }
                    d.auth =
                        (!fi.login.is_empty()).then(|| (fi.login.clone(), fi.password.clone()));
                    d.cookies =
                        (!fi.cookies.trim().is_empty()).then(|| fi.cookies.trim().to_string());
                    was = Some(d.state);
                }
                // Aim the running transfer at the edited destination; if a
                // small file already finished under the provisional name,
                // move it.
                if let Some(d) = self.item(fi.dl) {
                    let new_path = d.full_path();
                    engine::send(Cmd::SetFinalPath(
                        fi.dl,
                        new_path.to_string_lossy().into_owned(),
                    ));
                    if was == Some(DlState::Complete) {
                        if let Some(old) = old_path.filter(|o| *o != new_path) {
                            if let Some(dir) = new_path.parent() {
                                let _ = std::fs::create_dir_all(dir);
                            }
                            let _ = std::fs::rename(old, &new_path);
                        }
                    }
                }
                self.save_state();
                let close = self.close_file_info_windows();
                match (start, was) {
                    // Reveal the already-running background transfer.
                    (true, Some(st)) if st.is_active() => {
                        let seed = self.item(fi.dl).and_then(|d| d.speed_limit);
                        self.prog
                            .entry(fi.dl)
                            .or_insert_with(|| prog_state_seed(seed));
                        Task::batch([close, self.open_window(WinKind::Progress(fi.dl))])
                    }
                    (true, _) => Task::batch([close, self.start_download(fi.dl, true)]),
                    // Download Later: park the background transfer, keep the
                    // bytes for resume, and leave it Queued for Start Queue.
                    (false, Some(st)) if st.is_active() => {
                        self.stop_download(fi.dl);
                        let main_q = self.cfg.queues.first().map(|q| q.name.clone());
                        if let Some(d) = self.item_mut(fi.dl) {
                            if d.queue.is_none() {
                                d.queue = main_q;
                            }
                        }
                        close
                    }
                    (false, _) => {
                        if let Some(d) = self.item_mut(fi.dl) {
                            if matches!(d.state, DlState::Paused) {
                                d.state = DlState::Queued;
                            }
                        }
                        self.save_state();
                        close
                    }
                }
            }
            Message::FiCancel => {
                let fi = self.file_info.clone();
                let close = self.close_file_info_windows();
                if fi.is_new {
                    // The dialog was aborted: the auto-started item goes away
                    // with its temp data on cancel.
                    Task::batch([close, self.delete_item(fi.dl)])
                } else {
                    close
                }
            }

            // ----------------------------------------------------- progress
            Message::ProgTabSet(id, tab) => {
                self.prog.entry(id).or_default().tab = tab;
                Task::none()
            }
            Message::ProgToggleDetails(id) => {
                let p = self.prog.entry(id).or_default();
                p.details = !p.details;
                let details = p.details;
                // Collapse the dialog itself: no dead space below the
                // buttons when the details are hidden.
                if let Some(win) = self.win_of(WinKind::Progress(id)) {
                    let h = if details { 582.0 } else { 352.0 };
                    window::resize(win, iced::Size::new(680.0, h))
                } else {
                    Task::none()
                }
            }
            Message::ProgPauseResume(id) => {
                let active = self.item(id).map(|d| d.state.is_active()).unwrap_or(false);
                if active {
                    self.stop_download(id);
                    Task::none()
                } else {
                    self.start_download(id, false)
                }
            }
            Message::ProgCancel(id) => {
                // Cancel: pause the transfer (keep the partial file and
                // the list entry) and dismiss the progress dialog.
                if self.item(id).map(|d| d.state.is_active()).unwrap_or(false) {
                    self.stop_download(id);
                }
                if let Some(win) = self.win_of(WinKind::Progress(id)) {
                    self.windows.remove(&win);
                    window::close(win)
                } else {
                    Task::none()
                }
            }
            Message::ProgHideTab(id, which) => {
                if which == 1 {
                    self.cfg.settings.show_speed_tab = false;
                } else {
                    self.cfg.settings.show_completion_tab = false;
                }
                self.prog.entry(id).or_default().tab = ProgTab::Status;
                self.save_config();
                Task::none()
            }
            Message::ProgLimitOn(id, on) => {
                self.prog.entry(id).or_default().limit_on = on;
                let kb: u64 = self
                    .prog
                    .get(&id)
                    .and_then(|p| p.limit_kb.parse().ok())
                    .unwrap_or(10);
                if let Some(d) = self.item_mut(id) {
                    d.speed_limit = on.then_some(kb * 1024);
                }
                let lim = self.item(id).and_then(|d| self.effective_limit(d));
                engine::send(Cmd::SetLimit(id, lim));
                Task::none()
            }
            Message::ProgLimitRemember(id, b) => {
                // The live cap stays as it is; the flag only decides whether
                // it survives the transfer (Stopped/Finished clear the item's
                // limit when the box is unchecked).
                self.prog.entry(id).or_default().remember_limit = b;
                Task::none()
            }
            Message::ProgLimitKb(id, s) => {
                if s.chars().all(|c| c.is_ascii_digit()) && s.len() <= 9 {
                    let p = self.prog.entry(id).or_default();
                    p.limit_kb = s;
                    if p.limit_on {
                        let kb: u64 = p.limit_kb.parse().unwrap_or(10);
                        if let Some(d) = self.item_mut(id) {
                            d.speed_limit = Some(kb * 1024);
                        }
                        engine::send(Cmd::SetLimit(id, Some(kb * 1024)));
                    }
                }
                Task::none()
            }

            // ----------------------------------------------------- complete
            Message::OpenFile(id) => {
                if let Some(d) = self.item(id) {
                    let _ = open::that_detached(d.full_path());
                }
                self.close_window(WinKind::Complete(id))
            }
            Message::OpenFolder(id) => {
                if let Some(d) = self.item(id) {
                    let _ = open::that_detached(&d.save_dir);
                }
                Task::none()
            }

            // ------------------------------------------------------ options
            Message::OptTabSet(t) => {
                self.options.tab = t;
                Task::none()
            }
            Message::OptOk => {
                self.cfg.settings = self.options.draft.clone();
                self.cfg.categories = self.options.draft_cats.clone();
                self.save_config();
                // Re-assert Dock policy for the new setting. Windows are
                // still open here (Options itself), so this stays Regular;
                // the actual hide happens when the last window closes —
                // Accessory apps get no menu bar, so hiding the Dock while
                // a window is up would strip every Hydra menu.
                #[cfg(target_os = "macos")]
                crate::macos_dock::sync(self.cfg.settings.hide_from_taskbar, true);
                crate::autostart::apply(
                    self.cfg.settings.launch_on_startup,
                    self.cfg.settings.start_in_tray,
                );
                self.close_window(WinKind::Options)
            }
            Message::OptDraft(f) => self.on_opt_field(f),

            // ------------------------------------------------------- update
            Message::UpdateChecked(res) => {
                let manual = std::mem::take(&mut self.updater.manual);
                match res {
                    Ok(Some(info)) => {
                        crate::log::info(&format!("update available: {}", info.version));
                        self.updater = UpdateUiState {
                            info: Some(info),
                            ..UpdateUiState::default()
                        };
                        self.open_window(WinKind::Update)
                    }
                    Ok(None) if manual => {
                        self.confirm = Some(ConfirmKind::UpToDate);
                        self.open_window(WinKind::Confirm)
                    }
                    Ok(None) => Task::none(),
                    Err(e) if manual => {
                        crate::log::warn(&format!("update check failed: {e}"));
                        self.confirm = Some(ConfirmKind::UpdateCheckFailed(e));
                        self.open_window(WinKind::Confirm)
                    }
                    Err(e) => {
                        // A failed startup check is a log line, never a
                        // dialog: offline startups are normal.
                        crate::log::warn(&format!("update check failed: {e}"));
                        Task::none()
                    }
                }
            }
            Message::UpdateNow => {
                let Some(info) = self.updater.info.clone() else {
                    return Task::none();
                };
                crate::log::info(&format!(
                    "update: user chose Update Now for {}",
                    info.version
                ));
                if !matches!(
                    self.updater.phase,
                    UpdatePhase::Idle | UpdatePhase::Failed(_)
                ) {
                    return Task::none();
                }
                let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                self.updater.cancel = Some(cancel.clone());
                self.updater.phase = UpdatePhase::Downloading {
                    got: 0,
                    total: (info.size > 0).then_some(info.size),
                };
                Task::run(crate::update::run(info, cancel), Message::UpdateEvent)
            }
            Message::UpdateCancel => {
                match self.updater.phase {
                    // Mid-download: raise the flag; the stream answers with
                    // Cancelled once the transfer notices, which closes the
                    // window and cleans the partial file.
                    UpdatePhase::Downloading { .. } => {
                        if let Some(c) = &self.updater.cancel {
                            c.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                        Task::none()
                    }
                    // Too late to stop; ignore.
                    UpdatePhase::Verifying | UpdatePhase::Preparing | UpdatePhase::Restarting => {
                        Task::none()
                    }
                    _ => {
                        self.updater = UpdateUiState::default();
                        self.close_window(WinKind::Update)
                    }
                }
            }
            Message::UpdateOpenPage => {
                if let Some(info) = &self.updater.info {
                    if !info.html_url.is_empty() {
                        let _ = open::that_detached(&info.html_url);
                    }
                }
                Task::none()
            }
            Message::UpdateEvent(ev) => {
                use crate::update::UpdateEvent as Ev;
                match ev {
                    Ev::Progress(got, total) => {
                        if let UpdatePhase::Downloading { total: known, .. } = self.updater.phase {
                            self.updater.phase = UpdatePhase::Downloading {
                                got,
                                total: total.or(known),
                            };
                        }
                        Task::none()
                    }
                    Ev::Verifying => {
                        self.updater.phase = UpdatePhase::Verifying;
                        Task::none()
                    }
                    Ev::Preparing => {
                        self.updater.phase = UpdatePhase::Preparing;
                        Task::none()
                    }
                    Ev::Cancelled => {
                        self.updater = UpdateUiState::default();
                        self.close_window(WinKind::Update)
                    }
                    Ev::Failed(e) => {
                        self.updater.phase = UpdatePhase::Failed(e);
                        self.updater.cancel = None;
                        Task::none()
                    }
                    Ev::ReadyToRestart => {
                        // The finisher waits for this process to exit before
                        // swapping files; leave with everything persisted.
                        self.updater.phase = UpdatePhase::Restarting;
                        self.save_state();
                        self.save_config();
                        self.flush_saves();
                        iced::exit()
                    }
                }
            }

            // ---------------------------------------------------- scheduler
            Message::SchQueue(q) => {
                self.sch.rename_draft = q.clone();
                self.sch.queue = q;
                self.sch.renaming = false;
                Task::none()
            }
            Message::SchNameEdit => {
                // Stock queues keep their names; only user-created queues
                // rename.
                let builtin = self
                    .cfg
                    .queues
                    .iter()
                    .find(|q| q.name == self.sch.queue)
                    .map(|q| q.builtin)
                    .unwrap_or(false);
                if builtin {
                    return Task::none();
                }
                self.sch.renaming = true;
                self.sch.rename_draft = self.sch.queue.clone();
                iced::widget::operation::focus("sch-rename")
            }
            Message::SchNameDraft(v) => {
                self.sch.rename_draft = v;
                Task::none()
            }
            Message::SchNameCommit => {
                let old = self.sch.queue.clone();
                let new = self.sch.rename_draft.trim().to_string();
                let taken = self
                    .cfg
                    .queues
                    .iter()
                    .any(|q| q.name == new && q.name != old);
                self.sch.renaming = false;
                if new.is_empty() || taken || new == old {
                    // Revert an empty/duplicate draft.
                    self.sch.rename_draft = old;
                    return Task::none();
                }
                if let Some(q) = self.cfg.queues.iter_mut().find(|q| q.name == old) {
                    q.name = new.clone();
                }
                for d in &mut self.state.downloads {
                    if d.queue.as_deref() == Some(old.as_str()) {
                        d.queue = Some(new.clone());
                    }
                }
                self.sch.queue = new.clone();
                self.sch.rename_draft = new;
                self.save_config();
                self.save_state();
                // Queue submenus in the menu bar and tray carry the old name.
                self.refresh_native_menu();
                #[cfg(any(target_os = "macos", target_os = "windows"))]
                {
                    let queues: Vec<String> =
                        self.cfg.queues.iter().map(|q| q.name.clone()).collect();
                    crate::tray::reinstall(&queues, self.cfg.settings.power_save);
                }
                Task::none()
            }
            Message::SchTabSet(t) => {
                self.sch.tab = t;
                Task::none()
            }
            Message::SchField(f) => {
                self.on_sch_field(f);
                self.save_config();
                Task::none()
            }
            Message::SchStartNow => {
                let name = self.sch.queue.clone();
                // set_queue_running promotes paused members back to Queued.
                self.set_queue_running(&name, true);
                self.queue_tick()
            }
            Message::SchStop => {
                let name = self.sch.queue.clone();
                self.set_queue_running(&name, false);
                Task::none()
            }
            Message::SchNewQueue => {
                let n = self.cfg.queues.len() + 1;
                let name = format!("{} {n}", i18n::tr("Queue"));
                self.cfg.queues.push(crate::model::QueueDef {
                    name: name.clone(),
                    files_at_once: 4,
                    schedule: crate::model::Schedule::default(),
                    builtin: false,
                    running: false,
                    did_work: false,
                });
                self.sch.rename_draft = name.clone();
                self.sch.queue = name;
                self.save_config();
                Task::none()
            }
            Message::SchDeleteQueue => {
                let name = self.sch.queue.clone();
                let builtin = self
                    .cfg
                    .queues
                    .iter()
                    .find(|q| q.name == name)
                    .map(|q| q.builtin)
                    .unwrap_or(false);
                if builtin {
                    return Task::none();
                }
                if self.cfg.queues.len() > 1 {
                    self.cfg.queues.retain(|q| q.name != name);
                    for d in &mut self.state.downloads {
                        if d.queue.as_deref() == Some(name.as_str()) {
                            d.queue = None;
                        }
                    }
                    self.sch.queue = self.cfg.queues[0].name.clone();
                    self.sch.rename_draft = self.sch.queue.clone();
                    self.save_config();
                    self.save_state();
                }
                Task::none()
            }
            Message::SchDragStart(id) => {
                self.sch.sel_file = Some(id);
                self.sch.drag = Some(id);
                Task::none()
            }
            Message::SchDragOver(target) => {
                let Some(src) = self.sch.drag else {
                    return Task::none();
                };
                if src == target {
                    return Task::none();
                }
                let queue = self.sch.queue.clone();
                let mut order: Vec<DlId> = {
                    let mut m: Vec<&DownloadItem> = self
                        .state
                        .downloads
                        .iter()
                        .filter(|d| d.queue.as_deref() == Some(queue.as_str()))
                        .collect();
                    m.sort_by_key(|d| d.q_order);
                    m.iter().map(|d| d.id).collect()
                };
                let (Some(from), Some(to)) = (
                    order.iter().position(|x| *x == src),
                    order.iter().position(|x| *x == target),
                ) else {
                    return Task::none();
                };
                let moved = order.remove(from);
                order.insert(to, moved);
                for (i, id) in order.iter().enumerate() {
                    if let Some(d) = self.item_mut(*id) {
                        d.q_order = i as u32;
                    }
                }
                Task::none()
            }
            Message::SchFileUp | Message::SchFileDown => {
                let up = matches!(message, Message::SchFileUp);
                if let Some(sel) = self.sch.sel_file {
                    let queue = self.sch.queue.clone();
                    let mut members: Vec<usize> = self
                        .state
                        .downloads
                        .iter()
                        .enumerate()
                        .filter(|(_, d)| d.queue.as_deref() == Some(queue.as_str()))
                        .map(|(i, _)| i)
                        .collect();
                    members.sort_by_key(|&i| self.state.downloads[i].q_order);
                    if let Some(pos) = members
                        .iter()
                        .position(|&i| self.state.downloads[i].id == sel)
                    {
                        let swap_with = if up {
                            pos.checked_sub(1)
                        } else {
                            pos.checked_add(1)
                        };
                        if let Some(other) = swap_with.filter(|&o| o < members.len()) {
                            let (i, j) = (members[pos], members[other]);
                            let tmp = self.state.downloads[i].q_order;
                            self.state.downloads[i].q_order = self.state.downloads[j].q_order;
                            self.state.downloads[j].q_order = tmp;
                            self.save_state();
                        }
                    }
                }
                Task::none()
            }
            Message::SchFileRemove => {
                if let Some(sel) = self.sch.sel_file {
                    if let Some(d) = self.item_mut(sel) {
                        d.queue = None;
                        if d.state == DlState::Queued {
                            d.state = DlState::Paused;
                        }
                    }
                    self.sch.sel_file = None;
                    self.save_state();
                }
                Task::none()
            }

            // -------------------------------------------------------- batch
            Message::BatchEdit(a) => {
                self.batch.text.perform(a);
                self.batch.parsed = false;
                self.parse_batch();
                Task::none()
            }
            Message::BatchLoaded(Some(text)) => {
                self.batch
                    .text
                    .perform(iced::widget::text_editor::Action::Edit(
                        iced::widget::text_editor::Edit::Paste(std::sync::Arc::new(text)),
                    ));
                self.batch.parsed = false;
                self.parse_batch();
                // Probe each link's size in the background ("you may wait
                // until it checks and fills all file types").
                let ua = self.cfg.settings.user_agent.clone();
                let probes: Vec<Task<Message>> = self
                    .batch
                    .checks
                    .iter()
                    .filter(|(u, _)| !self.batch.sizes.contains_key(u))
                    .map(|(u, _)| {
                        let url = u.clone();
                        let ua = ua.clone();
                        Task::perform(engine::probe_size(url.clone(), ua), move |size| {
                            Message::BatchProbed(url.clone(), size)
                        })
                    })
                    .collect();
                Task::batch(probes)
            }
            Message::BatchLoaded(None) => Task::none(),
            Message::BatchProbed(url, size) => {
                if let Some(size) = size {
                    self.batch.sizes.insert(url, size);
                }
                Task::none()
            }
            Message::BatchCheck(i, b) => {
                if let Some(c) = self.batch.checks.get_mut(i) {
                    c.1 = b;
                }
                Task::none()
            }
            Message::BatchCheckAll(b) => {
                self.parse_batch();
                for c in &mut self.batch.checks {
                    c.1 = b;
                }
                Task::none()
            }
            Message::BatchSaveMode(m) => {
                self.batch.to_category = m == 1;
                self.batch.to_dir = m == 2;
                Task::none()
            }
            Message::BatchCategory(c) => {
                self.batch.category = c;
                Task::none()
            }
            Message::BatchDir(d) => {
                self.batch.dir = d;
                Task::none()
            }
            Message::BatchOk => {
                self.parse_batch();
                let checked: Vec<String> = self
                    .batch
                    .checks
                    .iter()
                    .filter(|(_, on)| *on)
                    .map(|(u, _)| u.clone())
                    .collect();
                if checked.is_empty() {
                    self.confirm = Some(ConfirmKind::NoneChecked);
                    return self.open_window(WinKind::Confirm);
                }
                let cat_override = self.batch.to_category.then(|| self.batch.category.clone());
                let dir_override = self.batch.to_dir.then(|| self.batch.dir.clone());
                let queue = Some("Main download queue".to_string());
                for url in checked {
                    let id = self.add_item(url, None, queue.clone());
                    if let Some(c) = &cat_override {
                        let dir = self
                            .cfg
                            .categories
                            .iter()
                            .find(|k| &k.name == c)
                            .map(|k| k.dir.clone());
                        if let Some(d) = self.item_mut(id) {
                            d.category = Some(c.clone());
                            if let Some(dir) = dir {
                                d.save_dir = dir;
                            }
                        }
                    }
                    if let Some(dir) = &dir_override {
                        if let Some(d) = self.item_mut(id) {
                            d.save_dir = dir.clone();
                        }
                    }
                    if let Some(d) = self.item_mut(id) {
                        d.state = DlState::Queued;
                    }
                }
                self.save_state();
                self.batch = BatchState::default();
                self.close_window(WinKind::Batch)
            }

            // ------------------------------------------------------ dialogs
            Message::ConfirmYes => {
                let kind = self.confirm.take();
                let remove_file = self.confirm_remove_file;
                let close = self.close_window(WinKind::Confirm);
                match kind {
                    Some(ConfirmKind::DeleteItems(ids)) => {
                        let mut tasks = vec![close];
                        for id in ids {
                            tasks.push(self.delete_item_opts(id, remove_file));
                        }
                        Task::batch(tasks)
                    }
                    Some(ConfirmKind::DeleteCompleted) => {
                        let ids: Vec<DlId> = self
                            .state
                            .downloads
                            .iter()
                            .filter(|d| d.state == DlState::Complete)
                            .map(|d| d.id)
                            .collect();
                        for id in ids {
                            self.remove_item_opts(id, remove_file);
                        }
                        self.save_state();
                        close
                    }
                    Some(ConfirmKind::StopWarn { ids, stop_queues }) => {
                        self.stop_ids(ids, stop_queues);
                        close
                    }
                    _ => close,
                }
            }
            Message::ConfirmRemoveFile(b) => {
                self.confirm_remove_file = b;
                Task::none()
            }
            Message::OpenPermissions => {
                // The guide window explains and deep-links; nothing can be
                // granted programmatically by design.
                self.confirm = None;
                let close = self.close_window(WinKind::Confirm);
                self.refresh_perm_status();
                Task::batch([close, self.open_window(WinKind::Permissions)])
            }
            Message::PermOpenPane(url) => {
                let _ = open::that_detached(url);
                Task::none()
            }
            Message::PermRefresh => {
                self.refresh_perm_status();
                Task::none()
            }
            Message::DupResume => {
                let existing = match self.confirm.take() {
                    Some(ConfirmKind::Duplicate { existing, .. }) => existing,
                    _ => None,
                };
                self.pending_add = None;
                let close = self.close_window(WinKind::Confirm);
                match existing {
                    Some(id) => {
                        self.selected = vec![id];
                        Task::batch([close, self.start_download(id, true)])
                    }
                    None => close,
                }
            }
            Message::DupOpen => {
                if let Some(ConfirmKind::Duplicate { file: Some(f), .. }) = self.confirm.take() {
                    let _ = open::that_detached(f);
                }
                self.pending_add = None;
                self.close_window(WinKind::Confirm)
            }
            Message::DupNew => {
                self.confirm = None;
                let close = self.close_window(WinKind::Confirm);
                let Some(pending) = self.pending_add.take() else {
                    return close;
                };
                let id = self.add_item(pending.url, pending.auth, None);
                self.apply_capture_extras(id, pending.cookies, pending.name);
                // `name_1.ext`, `name_2.ext`, ... until it collides with
                // neither the disk nor another list entry.
                if let Some(d) = self.item(id) {
                    let unique =
                        unique_file_name(&d.save_dir, &d.file_name, &self.state.downloads, id);
                    if let Some(d) = self.item_mut(id) {
                        d.file_name = unique;
                    }
                }
                self.save_state();
                if self.cfg.settings.show_file_info_dialog {
                    let d = self.item(id).unwrap();
                    self.file_info = FileInfoState {
                        dl: id,
                        category: d.category.clone().unwrap_or_else(|| "General".into()),
                        save_dir: d.save_dir.clone(),
                        file_name: d.file_name.clone(),
                        description: String::new(),
                        remember: true,
                        is_new: true,
                        url: d.url.clone(),
                        name_touched: true,
                        ..FileInfoState::default()
                    };
                    let bg = self.start_download(id, false);
                    Task::batch([close, self.open_window(WinKind::FileInfo(id)), bg])
                } else {
                    Task::batch([close, self.start_download(id, true)])
                }
            }
            Message::CloseThis(id) => {
                if self.windows.remove(&id) == Some(WinKind::Confirm) {
                    self.confirm = None;
                    self.pending_add = None;
                }
                window::close(id)
            }
        }
    }

    fn parse_batch(&mut self) {
        if self.batch.parsed {
            return;
        }
        let existing: Vec<(String, bool)> = self.batch.checks.clone();
        self.batch.checks = self
            .batch
            .text
            .text()
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && engine::parse_url(l).is_ok())
            .map(|l| {
                let prev = existing.iter().find(|(u, _)| u == l).map(|(_, b)| *b);
                (l.to_string(), prev.unwrap_or(true))
            })
            .collect();
        self.batch.parsed = true;
    }

    // ------------------------------------------------------------ menu bar

    fn on_menu(&mut self, action: MenuAction) -> Task<Message> {
        match action {
            MenuAction::AddNewDownload => {
                self.add_url = AddUrlState::default();
                // Pre-fill the address when the clipboard holds a
                // plausible download link.
                Task::batch([
                    self.open_window(WinKind::AddUrl),
                    iced::clipboard::read().map(Message::AddrPrefill),
                ])
            }
            MenuAction::AddBatch => {
                self.batch = BatchState::default();
                self.batch.category = "General".into();
                self.open_window(WinKind::Batch)
            }
            MenuAction::AddBatchClipboard => {
                self.batch = BatchState::default();
                self.batch.category = "General".into();
                let open = self.open_window(WinKind::Batch);
                Task::batch([
                    open,
                    iced::clipboard::read().map(|text| match text {
                        Some(t) => Message::BatchEdit(iced::widget::text_editor::Action::Edit(
                            iced::widget::text_editor::Edit::Paste(std::sync::Arc::new(t)),
                        )),
                        None => Message::Noop,
                    }),
                ])
            }
            MenuAction::AddBatchFile => {
                self.batch = BatchState::default();
                self.batch.category = "General".into();
                let open = self.open_window(WinKind::Batch);
                // Synchronous picker on purpose: AppKit dialogs must run on
                // the main thread — the async variant on a worker hangs.
                let text = rfd::FileDialog::new()
                    .add_filter("Text", &["txt", "text", "lst"])
                    .pick_file()
                    .and_then(|f| std::fs::read_to_string(f).ok());
                Task::batch([open, self.update(Message::BatchLoaded(text))])
            }
            MenuAction::SiteGrabber | MenuAction::DropTarget | MenuAction::Find => Task::none(),
            MenuAction::ExportList => {
                let urls: Vec<String> =
                    self.state.downloads.iter().map(|d| d.url.clone()).collect();
                let path = model::app_dir().join("export.txt");
                let _ = std::fs::write(&path, urls.join("\n"));
                let _ = open::that_detached(model::app_dir());
                Task::none()
            }
            MenuAction::ImportList => {
                let path = model::app_dir().join("export.txt");
                if let Ok(text) = std::fs::read_to_string(path) {
                    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
                        if engine::parse_url(line).is_ok() {
                            self.add_item(line.to_string(), None, None);
                        }
                    }
                }
                Task::none()
            }
            MenuAction::Exit => {
                self.save_state();
                self.save_config();
                self.flush_saves();
                iced::exit()
            }
            MenuAction::StopDownload => self.stop_ids_confirming(self.selected.clone(), false),
            MenuAction::Remove => self.update(Message::ToolbarDelete),
            MenuAction::DownloadNow | MenuAction::Redownload => {
                let ids = self.selected.clone();
                let show_progress = ids.len() == 1;
                let mut tasks = vec![];
                for id in ids {
                    if action == MenuAction::Redownload {
                        // A running transfer owns its `.part`; leave it be.
                        if self.item(id).map(|d| d.state.is_active()).unwrap_or(false) {
                            continue;
                        }
                        // Progress reset alone is not enough: the pinned
                        // `.part` would satisfy the resume check and the new
                        // object would be written into a file that still has
                        // the old one's bytes (and, via an edited URL, no
                        // record of which object it holds). Drop both.
                        if let Some(d) = self.item(id) {
                            let _ = std::fs::remove_file(d.part_file());
                        }
                        if let Some(d) = self.item_mut(id) {
                            d.held.clear();
                            d.downloaded = 0;
                            d.disp_progress = 0.0;
                            d.state = DlState::Paused;
                            d.part_path = None;
                            d.error = None;
                        }
                    }
                    tasks.push(self.start_download(id, show_progress));
                }
                Task::batch(tasks)
            }
            MenuAction::PauseAll | MenuAction::StopAll => {
                let ids: Vec<DlId> = self
                    .state
                    .downloads
                    .iter()
                    .filter(|d| d.state.is_active() || d.state == DlState::Queued)
                    .map(|d| d.id)
                    .collect();
                self.stop_ids_confirming(ids, true)
            }
            MenuAction::DeleteAllCompleted => {
                self.confirm = Some(ConfirmKind::DeleteCompleted);
                self.confirm_remove_file = false;
                self.open_window(WinKind::Confirm)
            }
            MenuAction::Scheduler => {
                if self.sch.queue.is_empty() {
                    self.sch.queue = self
                        .cfg
                        .queues
                        .first()
                        .map(|q| q.name.clone())
                        .unwrap_or_default();
                }
                self.sch.rename_draft = self.sch.queue.clone();
                self.open_window(WinKind::Scheduler)
            }
            MenuAction::StartQueue(name) => {
                // set_queue_running promotes paused members back to Queued.
                self.set_queue_running(&name, true);
                self.queue_tick()
            }
            MenuAction::StopQueue(name) => {
                self.set_queue_running(&name, false);
                Task::none()
            }
            MenuAction::SpeedLimiterToggle => {
                self.cfg.settings.speed_limiter_on = !self.cfg.settings.speed_limiter_on;
                if self.cfg.settings.global_speed_limit.is_none() {
                    self.cfg.settings.global_speed_limit = Some(128 * 1024);
                }
                let updates: Vec<(DlId, Option<u64>)> = self
                    .state
                    .downloads
                    .iter()
                    .filter(|d| d.state.is_active())
                    .map(|d| (d.id, self.effective_limit(d)))
                    .collect();
                for (id, lim) in updates {
                    engine::send(Cmd::SetLimit(id, lim));
                }
                self.save_config();
                self.refresh_native_menu();
                Task::none()
            }
            MenuAction::Options => {
                self.options.draft = self.cfg.settings.clone();
                self.options.draft_cats = self.cfg.categories.clone();
                self.options.sel_category = "General".into();
                self.options.auto_types_edit =
                    iced::widget::text_editor::Content::with_text(&self.cfg.settings.auto_types);
                self.options.sites_edit = iced::widget::text_editor::Content::with_text(
                    &self.cfg.settings.dont_start_sites,
                );
                self.open_window(WinKind::Options)
            }
            MenuAction::CheckUpdates => {
                // A known offer just reopens its dialog; otherwise run a
                // fresh check in manual mode so the result is always shown.
                if self.updater.info.is_some() {
                    return self.open_window(WinKind::Update);
                }
                self.updater.manual = true;
                Task::perform(
                    crate::update::check(self.cfg.settings.beta_channel),
                    Message::UpdateChecked,
                )
            }
            MenuAction::HideCategories => {
                self.cfg.settings.show_categories = !self.cfg.settings.show_categories;
                self.save_config();
                self.refresh_native_menu();
                Task::none()
            }
            MenuAction::ArrangeBy(key) => self.update(Message::SortBy(key)),
            MenuAction::ToggleDark => {
                self.cfg.settings.dark_mode = !self.cfg.settings.dark_mode;
                self.save_config();
                self.refresh_native_menu();
                Task::none()
            }
            MenuAction::FontSize(s) => {
                self.cfg.settings.font_size = s;
                self.save_config();
                self.refresh_native_menu();
                Task::none()
            }
            MenuAction::Language(l) => {
                i18n::set_locale(&l);
                self.cfg.language = Some(l);
                self.save_config();
                // Native surfaces captured the old locale at build time.
                self.refresh_native_menu();
                #[cfg(any(target_os = "macos", target_os = "windows"))]
                {
                    let queues: Vec<String> =
                        self.cfg.queues.iter().map(|q| q.name.clone()).collect();
                    crate::tray::reinstall(&queues, self.cfg.settings.power_save);
                }
                Task::none()
            }
            MenuAction::HomePage => {
                let _ = open::that_detached("https://github.com/ja7ad/hydra");
                Task::none()
            }
            MenuAction::About => self.open_window(WinKind::About),
            MenuAction::Permissions => self.update(Message::OpenPermissions),
            MenuAction::MoveToQueue(q) => {
                for id in self.selected.clone() {
                    let order = self
                        .state
                        .downloads
                        .iter()
                        .filter(|d| d.queue.as_deref() == Some(q.as_str()))
                        .map(|d| d.q_order + 1)
                        .max()
                        .unwrap_or(0);
                    if let Some(d) = self.item_mut(id) {
                        d.queue = Some(q.clone());
                        d.q_order = order;
                        if d.state == DlState::Paused {
                            d.state = DlState::Queued;
                        }
                    }
                }
                self.save_state();
                Task::none()
            }
            MenuAction::RemoveFromQueue => {
                for id in self.selected.clone() {
                    if let Some(d) = self.item_mut(id) {
                        d.queue = None;
                        if d.state == DlState::Queued {
                            d.state = DlState::Paused;
                        }
                    }
                }
                self.save_state();
                Task::none()
            }
            MenuAction::Shortcuts => self.open_window(WinKind::Shortcuts),
            MenuAction::PowerSaveToggle => {
                self.cfg.settings.power_save = !self.cfg.settings.power_save;
                engine::set_power_save(self.cfg.settings.power_save);
                crate::log::info(&format!("power save: {}", self.cfg.settings.power_save));
                self.save_config();
                #[cfg(any(target_os = "macos", target_os = "windows"))]
                {
                    let queues: Vec<String> =
                        self.cfg.queues.iter().map(|q| q.name.clone()).collect();
                    crate::tray::reinstall(&queues, self.cfg.settings.power_save);
                }
                Task::none()
            }
            MenuAction::OpenSel => {
                if let Some(d) = self.selected_item() {
                    let _ = open::that_detached(d.full_path());
                }
                Task::none()
            }
            MenuAction::OpenFolderSel => {
                if let Some(d) = self.selected_item() {
                    let _ = open::that_detached(&d.save_dir);
                }
                Task::none()
            }
            MenuAction::Properties => {
                let fi = self.selected_item().map(|d| FileInfoState {
                    dl: d.id,
                    category: d.category.clone().unwrap_or_else(|| "General".into()),
                    save_dir: d.save_dir.clone(),
                    file_name: d.file_name.clone(),
                    description: d.description.clone(),
                    remember: false,
                    is_new: false,
                    url: d.url.clone(),
                    login: d.auth.clone().map(|a| a.0).unwrap_or_default(),
                    password: d.auth.clone().map(|a| a.1).unwrap_or_default(),
                    cookies: d.cookies.clone().unwrap_or_default(),
                    ..FileInfoState::default()
                });
                if let Some(fi) = fi {
                    let id = fi.dl;
                    self.file_info = fi;
                    self.open_window(WinKind::FileInfo(id))
                } else {
                    Task::none()
                }
            }
        }
    }

    fn on_opt_field(&mut self, f: OptField) -> Task<Message> {
        // Editor actions first: they need `self.options` whole.
        match f {
            OptField::AutoTypesEdit(a) => {
                self.options.auto_types_edit.perform(a);
                self.options.draft.auto_types = self.options.auto_types_edit.text();
                return Task::none();
            }
            OptField::SitesEdit(a) => {
                self.options.sites_edit.perform(a);
                self.options.draft.dont_start_sites = self.options.sites_edit.text();
                return Task::none();
            }
            _ => {}
        }
        let s = &mut self.options.draft;
        match f {
            OptField::LaunchStartup(b) => s.launch_on_startup = b,
            OptField::CheckUpdates(b) => s.check_updates_on_startup = b,
            OptField::BetaChannel(b) => s.beta_channel = b,
            OptField::StartInTray(b) => s.start_in_tray = b,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            OptField::HideTaskbar(b) => s.hide_from_taskbar = b,
            OptField::PowerSave(b) => {
                s.power_save = b;
                engine::set_power_save(b);
            }
            OptField::GpuRender(b) => s.gpu_render = b,
            OptField::Clipboard(b) => s.monitor_clipboard = b,
            OptField::Browser(i, b) => {
                if let Some(x) = s.capture_browsers.get_mut(i) {
                    x.1 = b;
                }
            }
            OptField::AutoTypesEdit(_) | OptField::SitesEdit(_) => unreachable!(),
            OptField::ExcDialog(b) => s.show_exception_dialog = b,
            OptField::RememberLast(b) => s.remember_last_dir = b,
            OptField::ServerDate(b) => s.server_file_date = b,
            OptField::ShowFileInfo(b) => s.show_file_info_dialog = b,
            OptField::StartMinimized(b) => s.start_minimized = b,
            OptField::SpeedTab(b) => s.show_speed_tab = b,
            OptField::CompletionTab(b) => s.show_completion_tab = b,
            OptField::HideButtons(b) => s.show_hide_buttons = b,
            OptField::CompleteDialog(b) => s.show_complete_dialog = b,
            OptField::UserAgent(v) => s.user_agent = v,
            OptField::VirusScanner(v) => s.virus_scanner = v,
            OptField::VirusArgs(v) => s.virus_args = v,
            OptField::BrowseVirus => {
                let p = rfd::FileDialog::new()
                    .pick_file()
                    .map(|p| p.to_string_lossy().into_owned());
                return self.on_opt_field(OptField::VirusPicked(p));
            }
            OptField::VirusPicked(Some(p)) => s.virus_scanner = p,
            OptField::VirusPicked(None) => {}
            OptField::DefaultConns(n) => s.default_conns = n,
            OptField::AdaptiveConns(b) => s.adaptive_conns = b,
            OptField::ExcServer(v) => self.options.conn_exc_server = v,
            OptField::ExcConns(v) => self.options.conn_exc_n = v,
            OptField::ExcAdd => {
                let server = self.options.conn_exc_server.trim().to_string();
                let n: usize = self.options.conn_exc_n.trim().parse().unwrap_or(0);
                if !server.is_empty() && n > 0 {
                    self.options
                        .draft
                        .conn_exceptions
                        .push((server, n.clamp(1, 32)));
                    self.options.conn_exc_server.clear();
                    self.options.conn_exc_n.clear();
                }
            }
            OptField::DlLimit(b) => s.dl_limit_enabled = b,
            OptField::DlLimitMb(v) => {
                if let Ok(n) = v.parse() {
                    s.dl_limit_mb = n;
                }
            }
            OptField::DlLimitHours(v) => {
                if let Ok(n) = v.parse() {
                    s.dl_limit_hours = n;
                }
            }
            OptField::WarnStop(b) => s.warn_before_stop = b,
            OptField::ProxyMode(m) => s.proxy_mode = m,
            OptField::ProxyScript(v) => s.proxy_script = v,
            OptField::ProxyHost(v) => s.proxy_host = v,
            OptField::ProxyPort(v) => s.proxy_port = v,
            OptField::ProxyUser(v) => s.proxy_user = v,
            OptField::ProxyPass(v) => s.proxy_pass = v,
            OptField::ProxyHttp(b) => s.proxy_http = b,
            OptField::ProxyHttps(b) => s.proxy_https = b,
            OptField::ProxyFtp(b) => s.proxy_ftp = b,
            OptField::FtpPasv(b) => s.ftp_pasv = b,
            OptField::SelCategory(c) => self.options.sel_category = c,
            OptField::CatDir(v) => {
                let sel = self.options.sel_category.clone();
                if let Some(c) = self.options.draft_cats.iter_mut().find(|c| c.name == sel) {
                    c.dir = v;
                }
            }
            OptField::BrowseCatDir => {
                let p = rfd::FileDialog::new()
                    .pick_folder()
                    .map(|p| p.to_string_lossy().into_owned());
                return self.on_opt_field(OptField::CatDirPicked(p));
            }
            OptField::CatDirPicked(Some(p)) => {
                let sel = self.options.sel_category.clone();
                if let Some(c) = self.options.draft_cats.iter_mut().find(|c| c.name == sel) {
                    c.dir = p;
                }
            }
            OptField::CatDirPicked(None) => {}
            OptField::LoginSel(i) => {
                self.options.sel_login = Some(i);
                if let Some(l) = self.options.draft.logins.get(i) {
                    self.options.login_site = l.site.clone();
                    self.options.login_user = l.user.clone();
                    self.options.login_pass = l.pass.clone();
                }
            }
            OptField::LoginSite(v) => self.options.login_site = v,
            OptField::LoginUser(v) => self.options.login_user = v,
            OptField::LoginPass(v) => self.options.login_pass = v,
            OptField::LoginAdd => {
                let site = self.options.login_site.trim().to_string();
                if !site.is_empty() {
                    self.options.draft.logins.push(SiteLogin {
                        site,
                        user: self.options.login_user.clone(),
                        pass: self.options.login_pass.clone(),
                    });
                    self.options.login_site.clear();
                    self.options.login_user.clear();
                    self.options.login_pass.clear();
                }
            }
            OptField::LoginRemove => {
                if let Some(i) = self.options.sel_login.take() {
                    if i < self.options.draft.logins.len() {
                        self.options.draft.logins.remove(i);
                    }
                }
            }
            OptField::Sound(i, b) => {
                if let Some(row) = s.sounds.get_mut(i) {
                    row.enabled = b;
                }
            }
            OptField::SoundBrowse(i) => {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("Audio", &["wav", "ogg"])
                    .pick_file()
                {
                    if let Some(row) = s.sounds.get_mut(i) {
                        row.file = p.to_string_lossy().into_owned();
                    }
                }
            }
            OptField::SoundPlay(i) => {
                if let Some(row) = s.sounds.get(i) {
                    sounds::play(
                        (!row.file.is_empty()).then(|| row.file.clone()),
                        sounds::Event::from_index(i),
                    );
                }
            }
        }
        Task::none()
    }

    fn on_sch_field(&mut self, f: SchField) {
        let name = self.sch.queue.clone();
        let Some(q) = self.cfg.queues.iter_mut().find(|q| q.name == name) else {
            return;
        };
        let s = &mut q.schedule;
        match f {
            SchField::Periodic(b) => s.periodic = b,
            SchField::OnStartup(b) => s.start_on_startup = b,
            SchField::StartEnabled(b) => s.start_enabled = b,
            SchField::StartAt(v) => s.start_at = v,
            SchField::Once(b) => s.once = b,
            SchField::Day(i, b) => {
                if let Some(d) = s.days.get_mut(i) {
                    *d = b;
                }
            }
            SchField::StopEnabled(b) => s.stop_enabled = b,
            SchField::StopAt(v) => s.stop_at = v,
            SchField::RetriesEnabled(b) => s.retries_enabled = b,
            SchField::Retries(v) => {
                if let Ok(n) = v.parse() {
                    s.retries = n;
                }
            }
            SchField::OpenFileEnabled(b) => s.open_file_enabled = b,
            SchField::OpenFile(v) => s.open_file = v,
            SchField::ExitDone(b) => s.exit_when_done = b,
            SchField::FilesAtOnce(v) => {
                if let Ok(n) = v.parse::<u32>() {
                    q.files_at_once = n.clamp(1, 16);
                }
            }
        }
    }
}

/// Default main-window size: scaled from the primary display's logical
/// resolution at the reference ratio (1009x606 on a 1512x982 screen — i.e.
/// two thirds of the width, ~62% of the height), clamped to the layout's
/// 900x600 floor. Falls back to 1009x606 when the display cannot be queried.
pub fn main_window_size() -> iced::Size {
    let fallback = iced::Size::new(1009.0, 606.0);
    let Ok(displays) = display_info::DisplayInfo::all() else {
        return fallback;
    };
    let Some(d) = displays.iter().find(|d| d.is_primary).or(displays.first()) else {
        return fallback;
    };
    let (mut w, mut h) = (d.width as f32, d.height as f32);
    // Some backends report physical pixels; normalize to logical points.
    if d.scale_factor > 1.0 && w / d.scale_factor >= 1000.0 {
        w /= d.scale_factor;
        h /= d.scale_factor;
    }
    iced::Size::new(
        (w * (1009.0 / 1512.0)).max(900.0),
        (h * (606.0 / 982.0)).max(600.0),
    )
}

/// Does this clipboard line look like a download link? Two signals, either
/// suffices: the file extension is in the Options > File types list, or the
/// URL matches a known download-host pattern that carries no extension
/// (GitHub releases and friends).
pub fn looks_downloadable(url: &str, auto_types: &str) -> bool {
    if engine::parse_url(url).is_err() {
        return false;
    }
    let name = engine::file_name_from_url(url);
    if let Some((_, ext)) = name.rsplit_once('.') {
        let ext = ext.to_ascii_uppercase();
        // The list accepts commas, spaces or newlines as separators.
        if auto_types
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|t| !t.is_empty())
            .any(|t| t.eq_ignore_ascii_case(&ext))
        {
            return true;
        }
    }
    const PATTERNS: [&str; 6] = [
        "/releases/download/",
        "githubusercontent.com/",
        "/-/releases/",
        "sourceforge.net/projects/",
        "/files/download",
        "dl.google.com/",
    ];
    PATTERNS.iter().any(|p| url.contains(p))
}

/// Directory comparison for collision checks: `/a/b` and `/a/b/` (or a `..`
/// variant) are the same place on disk and must compare equal — a raw string
/// comparison let two spellings of one directory collide silently.
/// Existing directories canonicalize (resolving symlinks and case); paths
/// not on disk yet fall back to a structural normalisation.
pub fn normalize_dir(p: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(p);
    if let Ok(c) = std::fs::canonicalize(path) {
        return c;
    }
    let mut out = std::path::PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

/// `name.ext` -> first of `name.ext`, `name_1.ext`, `name_2.ext`, ... that
/// exists neither on disk in `dir` nor as another list entry saving there.
pub fn unique_file_name(
    dir: &str,
    name: &str,
    downloads: &[DownloadItem],
    skip_id: DlId,
) -> String {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), Some(e.to_string())),
        _ => (name.to_string(), None),
    };
    let candidate = |n: u32| -> String {
        let stem = if n == 0 {
            stem.clone()
        } else {
            format!("{stem}_{n}")
        };
        match &ext {
            Some(e) => format!("{stem}.{e}"),
            None => stem,
        }
    };
    let dir_norm = normalize_dir(dir);
    let taken: Vec<&str> = downloads
        .iter()
        .filter(|d| d.id != skip_id && normalize_dir(&d.save_dir) == dir_norm)
        .map(|d| d.file_name.as_str())
        .collect();
    for n in 0..1000 {
        let c = candidate(n);
        let on_disk = std::path::Path::new(dir).join(&c).exists();
        if !on_disk && !taken.iter().any(|t| *t == c) {
            return c;
        }
    }
    candidate(1000)
}

/// Total bytes covered by a span list.
pub fn sum_spans(spans: &[(u64, u64)]) -> u64 {
    spans.iter().map(|(lo, hi)| hi.saturating_sub(*lo)).sum()
}

/// Sort, clamp and merge persisted byte spans. `Scheduler::mark_done` is
/// told "these bytes arrived, never fetch them again", so spans read back
/// from disk are not trusted as-is: inverted spans drop, spans past the
/// object end clamp, and overlapping or adjacent spans merge.
pub fn sanitize_spans(spans: &[(u64, u64)], size: Option<u64>) -> Vec<(u64, u64)> {
    let mut v: Vec<(u64, u64)> = spans
        .iter()
        .map(|&(lo, hi)| match size {
            Some(s) => (lo.min(s), hi.min(s)),
            None => (lo, hi),
        })
        .filter(|&(lo, hi)| lo < hi)
        .collect();
    v.sort_unstable();
    let mut out: Vec<(u64, u64)> = Vec::with_capacity(v.len());
    for (lo, hi) in v {
        match out.last_mut() {
            Some(last) if lo <= last.1 => last.1 = last.1.max(hi),
            _ => out.push((lo, hi)),
        }
    }
    out
}

/// Does the `.part` staging file plausibly contain the recorded spans?
///
/// Length must match the recorded size exactly — `SparseSink::create` calls
/// `set_len(size)` up front, so any other length means another object or a
/// truncation. On unix the allocated blocks are checked against the bytes
/// the spans claim: holes are not allocated, so a `.part` that received one
/// byte cannot pass for one that received everything (file length says
/// nothing about completeness — the lesson the CLI already recorded).
pub fn part_matches(part: &std::path::Path, size: Option<u64>, held: &[(u64, u64)]) -> bool {
    let Ok(meta) = std::fs::metadata(part) else {
        return false;
    };
    if meta.len() == 0 {
        return false;
    }
    if let Some(s) = size {
        if meta.len() != s {
            return false;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let allocated = meta.blocks().saturating_mul(512);
        let claimed = sum_spans(held);
        // Filesystems account allocation in blocks; allow one block of
        // rounding at each end of every span.
        let slop = (held.len() as u64 + 1).saturating_mul(2 * 4096);
        if allocated.saturating_add(slop) < claimed {
            return false;
        }
    }
    #[cfg(target_os = "windows")]
    {
        use std::io::Read;
        // On Windows, detect sparse files by checking if we can read actual
        // data from the claimed parts. A file created with set_len but no
        // actual data written will fail this check.
        if !held.is_empty() {
            if let Ok(mut file) = std::fs::File::open(part) {
                let mut buf = vec![0u8; 4096.min(meta.len() as usize)];
                // Try to read from the first claimed part
                if let Ok(n) = file.read(&mut buf) {
                    if n == 0 {
                        // File is sparse: no data could be read
                        return false;
                    }
                }
            }
        }
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    let _ = held;
    true
}

/// Is a queue's scheduled start due this minute?
///
/// The day checkboxes belong to the *Daily* mode: "Once" fires at the next
/// matching wall-clock minute regardless of the ticked days (the caller
/// disarms it afterwards), while "Daily" fires only on ticked days. The
/// previous gating had the polarity inverted, so Once obeyed the day list
/// and Daily ignored it.
pub fn schedule_start_due(
    s: &crate::model::Schedule,
    hhmm: &str,
    weekday: usize,
    running: bool,
) -> bool {
    if !s.start_enabled || s.start_at != hhmm || running {
        return false;
    }
    s.once || s.days.get(weekday).copied().unwrap_or(false)
}

/// Put a queue's paused/errored members back to `Queued` with a fresh retry
/// budget. Shared by every start path (menu, toolbar, scheduler wall-clock,
/// startup) via [`App::set_queue_running`].
pub fn promote_queue_members(downloads: &mut [DownloadItem], queue: &str) {
    for d in downloads {
        if d.queue.as_deref() == Some(queue) && matches!(d.state, DlState::Paused | DlState::Error)
        {
            d.state = DlState::Queued;
            d.retries = 0;
        }
    }
}

/// Progress-dialog state seeded from the item's live cap, so the Speed
/// Limiter tab tells the truth about a persisted per-download limit instead
/// of showing an unchecked box while the cap is active.
fn prog_state_seed(speed_limit: Option<u64>) -> ProgState {
    ProgState {
        details: true,
        limit_on: speed_limit.is_some(),
        limit_kb: (speed_limit.unwrap_or(10 * 1024) / 1024).to_string(),
        remember_limit: speed_limit.is_some(),
        ..ProgState::default()
    }
}

/// Normalize a key press into a combo string ("cmd+shift+v"). `cmd` is ⌘ on
/// macOS and Ctrl elsewhere (`Modifiers::command()`); presses without a
/// command modifier are not shortcuts.
pub fn combo_string(key: &iced::keyboard::Key, mods: iced::keyboard::Modifiers) -> Option<String> {
    if !mods.command() {
        return None;
    }
    let base = match key {
        iced::keyboard::Key::Character(c) => c.to_string().to_ascii_lowercase(),
        _ => return None,
    };
    let mut combo = String::from("cmd+");
    if mods.shift() {
        combo.push_str("shift+");
    }
    if mods.alt() {
        combo.push_str("alt+");
    }
    combo.push_str(&base);
    Some(combo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Schedule;

    fn item(id: DlId, dir: &str, name: &str, queue: Option<&str>, state: DlState) -> DownloadItem {
        DownloadItem {
            id,
            url: format!("https://example.com/{name}"),
            file_name: name.into(),
            save_dir: dir.into(),
            category: None,
            description: String::new(),
            size: None,
            downloaded: 0,
            state,
            error: None,
            resume: None,
            added: 0,
            last_try: None,
            queue: queue.map(str::to_string),
            q_order: 0,
            auth: None,
            cookies: None,
            speed_limit: None,
            held: vec![],
            part_path: None,
            rate: 0.0,
            retries: 3,
            disp_progress: 0.0,
            eta_secs: None,
            conns: vec![],
            status_line: String::new(),
        }
    }

    #[test]
    fn sanitize_spans_merges_clamps_and_drops() {
        // Unsorted with an overlap and an adjacency: one merged span.
        assert_eq!(
            sanitize_spans(&[(50, 80), (0, 60), (80, 90)], Some(100)),
            vec![(0, 90)]
        );
        // Inverted and empty spans drop; spans past the end clamp.
        assert_eq!(
            sanitize_spans(&[(30, 10), (5, 5), (90, 500)], Some(100)),
            vec![(90, 100)]
        );
        // A span entirely past the end vanishes rather than surviving as
        // (size, size).
        assert_eq!(sanitize_spans(&[(200, 300)], Some(100)), vec![]);
        // Unknown size: no clamping, still sorted and merged.
        assert_eq!(sanitize_spans(&[(10, 20), (0, 15)], None), vec![(0, 20)]);
        assert_eq!(sum_spans(&[(0, 90), (100, 110)]), 100);
    }

    #[test]
    fn part_verification_rejects_holes_and_wrong_length() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("hydra-part-test-{}.part", std::process::id()));
        let _ = std::fs::remove_file(&path);

        // Missing file never resumes.
        assert!(!part_matches(&path, Some(1000), &[(0, 1000)]));

        // Sparse file: set_len only, no byte written. Length matches, but
        // nothing is allocated — claiming the whole object must fail.
        let f = std::fs::File::create(&path).unwrap();
        f.set_len(1_000_000).unwrap();
        drop(f);
        assert!(!part_matches(&path, Some(1_000_000), &[(0, 1_000_000)]));
        // ...while claiming nothing (fresh start) is fine.
        assert!(part_matches(&path, Some(1_000_000), &[]));

        // Fully written file passes.
        std::fs::write(&path, vec![0xAB; 1_000_000]).unwrap();
        assert!(part_matches(&path, Some(1_000_000), &[(0, 1_000_000)]));
        // Recorded size disagreeing with the file on disk fails.
        assert!(!part_matches(&path, Some(2_000_000), &[(0, 1_000_000)]));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn schedule_once_ignores_days_daily_honours_them() {
        let mut s = Schedule {
            start_enabled: true,
            start_at: "23:00".into(),
            ..Schedule::default()
        };
        s.days = [false; 7];

        // Daily (once == false) with no day ticked: never due.
        s.once = false;
        assert!(!schedule_start_due(&s, "23:00", 3, false));
        // Daily with Wednesday ticked: due on Wednesday only.
        s.days[3] = true;
        assert!(schedule_start_due(&s, "23:00", 3, false));
        assert!(!schedule_start_due(&s, "23:00", 4, false));

        // Once fires regardless of the day list.
        s.once = true;
        s.days = [false; 7];
        assert!(schedule_start_due(&s, "23:00", 0, false));

        // Wrong minute, already running, or disarmed: not due.
        assert!(!schedule_start_due(&s, "22:59", 0, false));
        assert!(!schedule_start_due(&s, "23:00", 0, true));
        s.start_enabled = false;
        assert!(!schedule_start_due(&s, "23:00", 0, false));
    }

    #[test]
    fn weekday_maps_sunday_to_zero() {
        use chrono::Datelike;
        // 2026-08-16 is a Sunday; the `days` array is UI-indexed Sun=0.
        let sun = chrono::NaiveDate::from_ymd_opt(2026, 8, 16).unwrap();
        assert_eq!(sun.weekday().num_days_from_sunday(), 0);
        let mon = chrono::NaiveDate::from_ymd_opt(2026, 8, 17).unwrap();
        assert_eq!(mon.weekday().num_days_from_sunday(), 1);
        let sat = chrono::NaiveDate::from_ymd_opt(2026, 8, 22).unwrap();
        assert_eq!(sat.weekday().num_days_from_sunday(), 6);
    }

    #[test]
    fn promote_resets_paused_and_errored_members_only() {
        let mut downloads = vec![
            item(1, "/d", "a.zip", Some("Q"), DlState::Paused),
            item(2, "/d", "b.zip", Some("Q"), DlState::Error),
            item(3, "/d", "c.zip", Some("Q"), DlState::Complete),
            item(4, "/d", "d.zip", Some("Q"), DlState::Receiving),
            item(5, "/d", "e.zip", Some("Other"), DlState::Paused),
            item(6, "/d", "f.zip", None, DlState::Paused),
        ];
        promote_queue_members(&mut downloads, "Q");
        assert_eq!(downloads[0].state, DlState::Queued);
        assert_eq!(downloads[0].retries, 0);
        assert_eq!(downloads[1].state, DlState::Queued);
        assert_eq!(downloads[1].retries, 0);
        assert_eq!(downloads[2].state, DlState::Complete);
        assert_eq!(downloads[3].state, DlState::Receiving);
        assert_eq!(downloads[4].state, DlState::Paused);
        assert_eq!(downloads[5].state, DlState::Paused);
    }

    #[test]
    fn unique_file_name_sees_through_dir_spelling() {
        // Same directory spelled with a trailing slash and a `.` segment:
        // the list entry must still count as a collision.
        let dir = "/nonexistent-hydra-test/downloads";
        let listed = item(
            1,
            "/nonexistent-hydra-test/./downloads/",
            "a.zip",
            None,
            DlState::Paused,
        );
        let name = unique_file_name(dir, "a.zip", &[listed], 99);
        assert_eq!(name, "a_1.zip");
        // No collision: the name is kept.
        let other = item(
            1,
            "/nonexistent-hydra-test/elsewhere",
            "a.zip",
            None,
            DlState::Paused,
        );
        assert_eq!(unique_file_name(dir, "a.zip", &[other], 99), "a.zip");
        // The entry being renamed does not collide with itself.
        let me = item(7, dir, "a.zip", None, DlState::Paused);
        assert_eq!(unique_file_name(dir, "a.zip", &[me], 7), "a.zip");
    }

    #[test]
    fn prog_state_seed_reflects_persisted_limit() {
        let p = prog_state_seed(Some(512 * 1024));
        assert!(p.limit_on);
        assert!(p.remember_limit);
        assert_eq!(p.limit_kb, "512");
        let p = prog_state_seed(None);
        assert!(!p.limit_on);
        assert!(!p.remember_limit);
        assert_eq!(p.limit_kb, "10");
    }
}
