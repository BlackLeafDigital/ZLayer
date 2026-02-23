//! Main application state machine and event loop
//!
//! The `App` struct manages all TUI state, delegates rendering and input
//! handling to the active screen/view, and drives the crossterm event loop.

use std::collections::HashMap;
use std::io::Stdout;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::prelude::*;
use ratatui::Terminal;

use crate::views;

// ---------------------------------------------------------------------------
// Screen enum -- which view is currently active
// ---------------------------------------------------------------------------

/// Top-level screen the app can be showing
pub enum Screen {
    /// Main menu with logo and navigation
    MainMenu(MainMenuState),
    /// Dashboard showing daemon status and deployments
    Dashboard(Box<DashboardState>),
    /// Multi-step deploy flow
    Deploy(Box<DeployState>),
    /// Multi-step build wizard
    BuildWizard(Box<BuildWizardState>),
    /// Runtime template browser
    RuntimeBrowser(RuntimeBrowserState),
    /// Dockerfile / ZImagefile validator
    Validate(ValidateState),
}

// ---------------------------------------------------------------------------
// Per-screen state types
// ---------------------------------------------------------------------------

/// State for the main menu
#[derive(Default)]
pub struct MainMenuState {
    /// Currently highlighted menu item index
    pub selected: usize,
}

/// Steps within the build wizard
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStep {
    SelectSource,
    Configure,
    Review,
    Building,
    Complete,
}

/// State for the build wizard
pub struct BuildWizardState {
    pub step: BuildStep,
    /// Path to Dockerfile / ZImagefile (or None to use runtime template)
    pub source_path: Option<PathBuf>,
    /// Selected runtime template name (alternative to source_path)
    pub runtime: Option<String>,
    /// Image tags to apply
    pub tags: Vec<String>,
    /// Build arguments (KEY=VALUE)
    pub build_args: HashMap<String, String>,
    /// Build context directory
    pub context_dir: PathBuf,
    /// Target stage for multi-stage builds
    pub target: Option<String>,
    /// Output format (oci / docker)
    pub format: String,
    /// Current text input buffer (for the active field)
    pub input_buf: String,
    /// Which configure field is focused
    pub config_field: usize,
    /// File picker state (for SelectSource step)
    pub file_picker: crate::widgets::file_picker::FilePickerState,
    /// Build result: image ID
    pub result_image_id: Option<String>,
    /// Build result: error message
    pub result_error: Option<String>,
    /// Build event receiver (populated during Building step)
    pub build_rx: Option<mpsc::Receiver<zlayer_builder::BuildEvent>>,
    /// Build state for rendering progress (populated during Building step)
    pub build_state: Option<zlayer_builder::tui::BuildState>,
}

impl BuildWizardState {
    pub fn new(context_dir: PathBuf) -> Self {
        let file_picker = crate::widgets::file_picker::FilePickerState::new(context_dir.clone());
        Self {
            step: BuildStep::SelectSource,
            source_path: None,
            runtime: None,
            tags: Vec::new(),
            build_args: HashMap::new(),
            context_dir,
            target: None,
            format: "oci".to_string(),
            input_buf: String::new(),
            config_field: 0,
            file_picker,
            result_image_id: None,
            result_error: None,
            build_rx: None,
            build_state: None,
        }
    }
}

/// State for the runtime browser
#[derive(Default)]
pub struct RuntimeBrowserState {
    /// Index of the selected runtime in the list
    pub selected: usize,
    /// Whether detail pane is open
    pub show_detail: bool,
}

/// State for the validate screen
pub struct ValidateState {
    /// Path input buffer
    pub path_input: String,
    /// Parsed result (populated after validation)
    pub result: Option<ValidateResult>,
    /// File picker for browsing
    pub file_picker: crate::widgets::file_picker::FilePickerState,
    /// Whether the file picker is active
    pub picker_active: bool,
}

/// Validation result
pub enum ValidateResult {
    /// Successfully parsed Dockerfile
    Dockerfile {
        path: String,
        stages: Vec<ValidateStageInfo>,
    },
    /// Successfully parsed ZImagefile
    ZImagefile { path: String, summary: String },
    /// Parse error
    Error { path: String, message: String },
}

/// Info about a parsed stage (for display)
pub struct ValidateStageInfo {
    pub index: usize,
    pub name: String,
    pub base_image: String,
    pub instruction_count: usize,
}

impl Default for ValidateState {
    fn default() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            path_input: String::new(),
            result: None,
            file_picker: crate::widgets::file_picker::FilePickerState::new(cwd),
            picker_active: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Dashboard state
// ---------------------------------------------------------------------------

/// State for the dashboard screen
pub struct DashboardState {
    /// Daemon connection status
    pub daemon_status: views::dashboard::DaemonStatus,
    /// List of deployments fetched from the daemon
    pub deployments: Vec<views::dashboard::DeploymentInfo>,
    /// Selected deployment index
    pub selected: usize,
    /// Detail view for a specific deployment (drill-down)
    pub detail_view: Option<views::dashboard::DeploymentDetail>,
    /// Selected service index within detail view
    pub detail_selected: usize,
    /// Timestamp of last data refresh
    pub last_refresh: Option<std::time::Instant>,
}

impl Default for DashboardState {
    fn default() -> Self {
        Self {
            daemon_status: views::dashboard::DaemonStatus::Connecting,
            deployments: Vec::new(),
            selected: 0,
            detail_view: None,
            detail_selected: 0,
            last_refresh: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Deploy state
// ---------------------------------------------------------------------------

/// State for the deploy screen
pub struct DeployState {
    /// Current step
    pub step: views::deploy::DeployStep,
    /// Spec files found in CWD
    pub spec_files: Vec<PathBuf>,
    /// Selected file index
    pub selected: usize,
    /// The file that was selected for deployment
    pub selected_file: Option<PathBuf>,
    /// Parsed deployment spec (populated after parse)
    pub parsed_spec: Option<zlayer_spec::DeploymentSpec>,
    /// Raw YAML content of the spec
    pub spec_yaml: Option<String>,
    /// Parse error message
    pub parse_error: Option<String>,
    /// Deploy error message
    pub deploy_error: Option<String>,
}

impl DeployState {
    pub fn new() -> Self {
        let spec_files = views::deploy::scan_spec_files();
        Self {
            step: views::deploy::DeployStep::SelectFile,
            spec_files,
            selected: 0,
            selected_file: None,
            parsed_spec: None,
            spec_yaml: None,
            parse_error: None,
            deploy_error: None,
        }
    }
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

/// Main application struct
pub struct App {
    /// Current screen
    pub screen: Screen,
    /// Whether the application should quit
    pub should_quit: bool,
    /// Whether the help overlay is visible
    pub show_help: bool,
    /// Initial context directory (from CLI args or cwd)
    pub initial_context: PathBuf,
    /// Tokio runtime for async daemon communication
    pub tokio_rt: tokio::runtime::Runtime,
}

impl App {
    /// Create a new App instance
    pub fn new(context: Option<PathBuf>) -> Self {
        let initial_context = context
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));

        let tokio_rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("zlayer-tui-rt")
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime for TUI");

        Self {
            screen: Screen::MainMenu(MainMenuState::default()),
            should_quit: false,
            show_help: false,
            initial_context,
            tokio_rt,
        }
    }

    /// Main event loop -- blocks until the user quits
    pub fn run(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> anyhow::Result<()> {
        while !self.should_quit {
            // If we are in the Building step, poll for build events
            self.poll_build_events();

            // If we are on the Dashboard, auto-refresh every 2 seconds
            self.poll_dashboard_refresh();

            // Render
            terminal.draw(|frame| self.render(frame))?;

            // Poll for keyboard / resize events
            if event::poll(Duration::from_millis(50))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        self.handle_key(key);
                    }
                    Event::Resize(_, _) => {
                        // Ratatui handles resize automatically on next draw
                    }
                    _ => {}
                }
            }
        }

        Ok(())
    }

    // ------------------------------------------------------------------
    // Rendering dispatch
    // ------------------------------------------------------------------

    fn render(&self, frame: &mut Frame) {
        if self.show_help {
            self.render_help_overlay(frame);
            return;
        }

        match &self.screen {
            Screen::MainMenu(state) => views::main_menu::render(frame, state),
            Screen::Dashboard(state) => views::dashboard::render(frame, state),
            Screen::Deploy(state) => views::deploy::render(frame, state),
            Screen::BuildWizard(state) => views::build::render(frame, state),
            Screen::RuntimeBrowser(state) => views::runtimes::render(frame, state),
            Screen::Validate(state) => views::validate::render(frame, state),
        }
    }

    fn render_help_overlay(&self, frame: &mut Frame) {
        use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

        let area = centered_rect(60, 60, frame.area());
        frame.render_widget(Clear, area);

        let help_text = "\
Keyboard Shortcuts
------------------

Navigation:
  Up / k        Move up
  Down / j      Move down
  Enter         Select / confirm
  Esc / q       Back / quit
  Tab           Next field (forms)
  ?             Toggle this help

Dashboard:
  r             Refresh data
  s             Start daemon
  Enter         Drill into deployment

Deploy:
  Enter         Select file / confirm
  Esc           Go back a step
  d             Go to dashboard (result)

Build Wizard:
  Enter         Confirm current step
  Backspace     Edit text fields
  Esc           Cancel / go back

General:
  Ctrl+C        Force quit
";

        let block = Block::default()
            .title(" Help ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));

        let paragraph = Paragraph::new(help_text)
            .block(block)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(Color::White));

        frame.render_widget(paragraph, area);
    }

    // ------------------------------------------------------------------
    // Input dispatch
    // ------------------------------------------------------------------

    fn handle_key(&mut self, key: KeyEvent) {
        // Global shortcuts
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        if key.code == KeyCode::Char('?') {
            self.show_help = !self.show_help;
            return;
        }

        if self.show_help {
            // Any key besides ? closes help
            self.show_help = false;
            return;
        }

        // Determine if we need to transition screens after handling the key.
        // We compute the transition target separately to avoid double-borrow.
        let transition: Option<Screen> = match &mut self.screen {
            Screen::MainMenu(state) => {
                handle_main_menu_key(key, state, &self.initial_context, &mut self.should_quit)
            }
            Screen::Dashboard(state) => {
                let go_back = views::dashboard::handle_key(key, state, &self.tokio_rt);
                if go_back {
                    Some(Screen::MainMenu(MainMenuState::default()))
                } else {
                    None
                }
            }
            Screen::Deploy(state) => {
                let action = views::deploy::handle_key(key, state, &self.tokio_rt);
                match action {
                    views::deploy::DeployAction::MainMenu => {
                        Some(Screen::MainMenu(MainMenuState::default()))
                    }
                    views::deploy::DeployAction::Dashboard => {
                        let mut ds = DashboardState::default();
                        views::dashboard::refresh_data(&self.tokio_rt, &mut ds);
                        Some(Screen::Dashboard(Box::new(ds)))
                    }
                    views::deploy::DeployAction::None => None,
                }
            }
            Screen::BuildWizard(state) => {
                views::build::handle_key(key, state);
                if key.code == KeyCode::Esc && state.step == BuildStep::SelectSource {
                    Some(Screen::MainMenu(MainMenuState::default()))
                } else {
                    None
                }
            }
            Screen::RuntimeBrowser(state) => {
                views::runtimes::handle_key(key, state);
                if key.code == KeyCode::Esc {
                    Some(Screen::MainMenu(MainMenuState::default()))
                } else {
                    None
                }
            }
            Screen::Validate(state) => {
                views::validate::handle_key(key, state);
                if key.code == KeyCode::Esc && state.result.is_none() && !state.picker_active {
                    Some(Screen::MainMenu(MainMenuState::default()))
                } else {
                    None
                }
            }
        };

        if let Some(new_screen) = transition {
            self.screen = new_screen;
        }
    }

    // ------------------------------------------------------------------
    // Dashboard auto-refresh
    // ------------------------------------------------------------------

    fn poll_dashboard_refresh(&mut self) {
        if let Screen::Dashboard(ref mut state) = self.screen {
            let should_refresh = match state.last_refresh {
                Some(last) => last.elapsed() >= Duration::from_secs(2),
                None => true, // First load
            };

            if should_refresh {
                if state.detail_view.is_some() {
                    let name = state
                        .detail_view
                        .as_ref()
                        .map(|d| d.deployment_name.clone())
                        .unwrap_or_default();
                    views::dashboard::refresh_detail(&self.tokio_rt, state, &name);
                } else {
                    views::dashboard::refresh_data(&self.tokio_rt, state);
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Build event polling
    // ------------------------------------------------------------------

    fn poll_build_events(&mut self) {
        if let Screen::BuildWizard(ref mut state) = self.screen {
            if state.step != BuildStep::Building {
                return;
            }

            if let Some(ref rx) = state.build_rx {
                let build_state = state
                    .build_state
                    .get_or_insert_with(zlayer_builder::tui::BuildState::default);

                // Drain all pending events
                loop {
                    match rx.try_recv() {
                        Ok(event) => {
                            use zlayer_builder::BuildEvent;
                            match event {
                                BuildEvent::StageStarted {
                                    index,
                                    name,
                                    base_image,
                                } => {
                                    use zlayer_builder::tui::StageState;
                                    while build_state.stages.len() <= index {
                                        build_state.stages.push(StageState {
                                            index: build_state.stages.len(),
                                            name: None,
                                            base_image: String::new(),
                                            instructions: Vec::new(),
                                            complete: false,
                                        });
                                    }
                                    build_state.stages[index] = StageState {
                                        index,
                                        name,
                                        base_image,
                                        instructions: Vec::new(),
                                        complete: false,
                                    };
                                    build_state.current_stage = index;
                                }
                                BuildEvent::InstructionStarted {
                                    stage,
                                    index,
                                    instruction,
                                } => {
                                    use zlayer_builder::tui::InstructionState;
                                    if let Some(s) = build_state.stages.get_mut(stage) {
                                        while s.instructions.len() <= index {
                                            s.instructions.push(InstructionState {
                                                text: String::new(),
                                                status: zlayer_builder::InstructionStatus::Pending,
                                            });
                                        }
                                        s.instructions[index] = InstructionState {
                                            text: instruction,
                                            status: zlayer_builder::InstructionStatus::Running,
                                        };
                                        build_state.current_instruction = index;
                                    }
                                }
                                BuildEvent::Output { line, is_stderr } => {
                                    use zlayer_builder::tui::OutputLine;
                                    build_state.output_lines.push(OutputLine {
                                        text: line,
                                        is_stderr,
                                    });
                                    let visible = 10usize;
                                    let max =
                                        build_state.output_lines.len().saturating_sub(visible);
                                    if build_state.scroll_offset >= max.saturating_sub(1) {
                                        build_state.scroll_offset = max;
                                    }
                                }
                                BuildEvent::InstructionComplete {
                                    stage,
                                    index,
                                    cached,
                                } => {
                                    if let Some(s) = build_state.stages.get_mut(stage) {
                                        if let Some(inst) = s.instructions.get_mut(index) {
                                            inst.status =
                                                zlayer_builder::InstructionStatus::Complete {
                                                    cached,
                                                };
                                        }
                                    }
                                }
                                BuildEvent::StageComplete { index } => {
                                    if let Some(s) = build_state.stages.get_mut(index) {
                                        s.complete = true;
                                    }
                                }
                                BuildEvent::BuildComplete { image_id } => {
                                    build_state.completed = true;
                                    build_state.image_id = Some(image_id.clone());
                                    state.result_image_id = Some(image_id);
                                    state.step = BuildStep::Complete;
                                }
                                BuildEvent::BuildFailed { error } => {
                                    build_state.completed = true;
                                    build_state.error = Some(error.clone());
                                    state.result_error = Some(error);
                                    state.step = BuildStep::Complete;
                                }
                            }
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            if !build_state.completed {
                                build_state.completed = true;
                                if build_state.error.is_none() && build_state.image_id.is_none() {
                                    let msg = "Build ended unexpectedly".to_string();
                                    build_state.error = Some(msg.clone());
                                    state.result_error = Some(msg);
                                }
                                state.step = BuildStep::Complete;
                            }
                            break;
                        }
                    }
                }
            }
        }
    }
}

/// Handle key events for the main menu (free function to avoid borrow issues)
fn handle_main_menu_key(
    key: KeyEvent,
    state: &mut MainMenuState,
    initial_context: &std::path::Path,
    should_quit: &mut bool,
) -> Option<Screen> {
    // Dashboard, Deploy, Build, Validate, Runtimes, Quit
    const MENU_ITEMS: usize = 6;

    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            state.selected = state.selected.saturating_sub(1);
            None
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if state.selected < MENU_ITEMS - 1 {
                state.selected += 1;
            }
            None
        }
        KeyCode::Enter => match state.selected {
            0 => Some(Screen::Dashboard(Box::default())),
            1 => Some(Screen::Deploy(Box::new(DeployState::new()))),
            2 => Some(Screen::BuildWizard(Box::new(BuildWizardState::new(
                initial_context.to_path_buf(),
            )))),
            3 => Some(Screen::Validate(ValidateState::default())),
            4 => Some(Screen::RuntimeBrowser(RuntimeBrowserState::default())),
            5 => {
                *should_quit = true;
                None
            }
            _ => None,
        },
        KeyCode::Char('q') | KeyCode::Esc => {
            *should_quit = true;
            None
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a centered rectangle of a given percentage of the parent area
pub fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
