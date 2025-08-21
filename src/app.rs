use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::{
    event::{self, poll, DisableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use itertools::{enumerate, Itertools};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    DefaultTerminal, Frame,
};

use crate::jellyfin::{Jellyfin, MediaItem};

pub struct App {
    jellyfin: Jellyfin,
    current_action: Action,
    page: Page,
    query: String,
    main_selection: Selection,
    episode_selection: Selection,
    selection_state: SelectionState,
    movies: Vec<MediaItem>,
    series: Vec<MediaItem>,
    episodes: Vec<MediaItem>,
    filtered: Vec<MediaItem>,
    config: Config,
}

struct Config {
    include_episodes: bool,
}

#[derive(PartialEq)]
enum Page {
    All,
    Movies,
    Series,
    Episodes,
    ContinueWatching,
    NextUp,
    LatestAdded,
    AllMovies,
    AllSeries,
}

#[derive(PartialEq)]
enum SelectionState {
    Main,
    Episode,
}

enum Action {
    None,
    NowPlaying(MediaItem),
    RefreshingCache,
}

#[derive(Clone)]
struct Selection {
    index: usize,
    scroll_position: usize,
    visible_height: usize,
    series: Option<MediaItem>,
    episodes: Option<Vec<MediaItem>>,
}

impl Selection {
    fn new() -> Self {
        Self {
            index: 0,
            scroll_position: 0,
            visible_height: 0,
            series: None,
            episodes: None,
        }
    }
}

impl App {
    pub fn new(jellyfin: Jellyfin) -> Result<Self> {
        let mut app = Self {
            jellyfin,
            current_action: Action::None,
            page: Page::ContinueWatching,
            query: String::new(),
            main_selection: Selection::new(),
            episode_selection: Selection::new(),
            selection_state: SelectionState::Main,
            movies: Vec::new(),
            series: Vec::new(),
            episodes: Vec::new(),
            filtered: Vec::new(),
            config: Config {
                include_episodes: false,
            },
        };

        app.movies = app
            .jellyfin
            .items
            .values()
            .filter(|item| item.type_ == "Movie")
            .cloned()
            .sorted_by(|a, b| a.name.cmp(&b.name))
            .collect();

        app.series = app
            .jellyfin
            .items
            .values()
            .filter(|item| item.type_ == "Series")
            .cloned()
            .sorted_by(|a, b| a.name.cmp(&b.name))
            .collect();

        app.episodes = app
            .jellyfin
            .items
            .values()
            .filter(|item| item.type_ == "Episode")
            .cloned()
            .sorted_by(|a, b| a.name.cmp(&b.name))
            .collect();

        Ok(app)
    }

    pub async fn run(
        &mut self,
        terminal: &mut DefaultTerminal,
        render_outer: impl Fn(&mut Frame) -> Rect,
    ) -> Result<()> {
        // init terminal
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;

        loop {
            self.draw(terminal, &render_outer)?;
            if self.handle_action().await? {
                continue;
            }
            if !self.handle_input()? {
                break;
            }
        }

        // cleanup
        self.jellyfin.cleanup()?;

        Ok(())
    }

    fn index(&self, state: Option<&SelectionState>) -> usize {
        match state.unwrap_or(&self.selection_state) {
            SelectionState::Main => self.main_selection.index,
            SelectionState::Episode => self.episode_selection.index,
        }
    }

    fn set_index(&mut self, index: usize) {
        match self.selection_state {
            SelectionState::Main => self.main_selection.index = index,
            SelectionState::Episode => self.episode_selection.index = index,
        }
    }

    fn scroll_position(&self, state: Option<&SelectionState>) -> usize {
        match state.unwrap_or(&self.selection_state) {
            SelectionState::Main => self.main_selection.scroll_position,
            SelectionState::Episode => self.episode_selection.scroll_position,
        }
    }

    fn selection_options(&self, state: Option<&SelectionState>) -> &Vec<MediaItem> {
        match state.unwrap_or(&self.selection_state) {
            SelectionState::Main => match self.page {
                Page::ContinueWatching => &self.jellyfin.continue_watching,
                Page::NextUp => &self.jellyfin.next_up,
                Page::LatestAdded => &self.jellyfin.latest_added,
                Page::AllMovies => &self.movies,
                Page::AllSeries => &self.series,
                _ => &self.filtered,
            },
            SelectionState::Episode => {
                if let Some(episodes) = &self.episode_selection.episodes {
                    episodes
                } else {
                    &self.filtered
                }
            }
        }
    }

    fn selected_item(&self) -> Option<MediaItem> {
        self.selection_options(None).get(self.index(None)).cloned()
    }

    fn search(&mut self) {
        let mut all;
        let pool = match self.page {
            Page::All => {
                all = self.movies.clone();
                all.extend(self.series.clone());
                if self.config.include_episodes {
                    all.extend(self.episodes.clone());
                }
                &all
            }
            Page::Movies => &self.movies,
            Page::Series => &self.series,
            Page::Episodes => &self.episodes,
            _ => return,
        };

        if self.query.is_empty() {
            self.filtered = pool.to_vec();
            return;
        }

        let matcher = SkimMatcherV2::default();

        self.filtered = pool
            .iter()
            .map(|item| {
                (
                    item,
                    matcher.fuzzy_match(&item.name, &self.query.to_lowercase()),
                )
            })
            .filter(|(_, score)| score.is_some())
            .sorted_by(|(_, a), (_, b)| b.cmp(a))
            .map(|(item, _)| item.clone())
            .collect();
    }

    fn draw(
        &mut self,
        terminal: &mut DefaultTerminal,
        render_outer: impl Fn(&mut Frame) -> Rect,
    ) -> Result<()> {
        terminal.draw(|frame| {
            let inner_area = render_outer(frame);
            let main_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
                .split(inner_area);

            self.draw_media_panel(frame, main_chunks[0], self.selected_item());

            let right_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(main_chunks[1]);

            let right_top_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(0)])
                .split(right_chunks[0]);

            self.draw_search_bar(frame, right_top_chunks[0]);

            match &self.selection_state {
                SelectionState::Main => {
                    // merge right chunks into one for single page
                    // ? right chunks are split beforehand so that they stay aligned with the left panel
                    self.draw_main(
                        frame,
                        ratatui::prelude::Rect {
                            x: right_top_chunks[1].x,
                            y: right_top_chunks[1].y,
                            width: right_top_chunks[1].width,
                            height: right_top_chunks[1].height + right_chunks[1].height,
                        },
                        SelectionState::Main,
                    )
                }
                SelectionState::Episode => {
                    self.draw_main(frame, right_top_chunks[1], SelectionState::Main);
                    self.draw_main(frame, right_chunks[1], SelectionState::Episode);
                }
            }

            self.draw_action(frame, inner_area);
        })?;

        Ok(())
    }

    fn handle_input(&mut self) -> Result<bool> {
        let Event::Key(key) = event::read()? else {
            return Ok(true);
        };

        match key.code {
            // ! make F1 show help
            KeyCode::Char('c') if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                return Ok(false);
            }
            KeyCode::F(5) => {
                self.current_action = Action::RefreshingCache;
            }
            KeyCode::Char('r') if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                self.current_action = Action::RefreshingCache;
            }
            KeyCode::Char('e') if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                self.config.include_episodes = !self.config.include_episodes;

                if self.page == Page::Episodes {
                    self.page = Page::All;
                }

                if self.page == Page::All {
                    self.search();
                }
            }
            KeyCode::Backspace | KeyCode::Char('h')
                if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
            {
                // ? ctrl+h is backspace on some terminals
                self.query.clear();
                self.page = Page::ContinueWatching;
                self.set_index(0);
                self.selection_state = SelectionState::Main;
                self.filtered.clear();
            }
            KeyCode::Char(c) => {
                if self.query.is_empty() {
                    self.page = Page::All;
                }

                self.query.push(c);
                self.set_index(0);
                self.selection_state = SelectionState::Main;
                self.search();
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.set_index(0);
                self.selection_state = SelectionState::Main;

                if !self.query.is_empty() {
                    self.search();
                } else {
                    self.page = Page::ContinueWatching;
                    self.filtered.clear();
                }
            }
            KeyCode::Enter => {
                let Some(item) = self.selected_item() else {
                    return Ok(true);
                };

                if item.type_ != "Series" {
                    self.current_action = Action::NowPlaying(item.clone());
                    return Ok(true);
                }

                self.selection_state = SelectionState::Episode;
                self.episode_selection.series = Some(item.clone());
                self.episode_selection.episodes =
                    Some(self.jellyfin.get_episodes_from_series(&item.id));
            }
            KeyCode::Esc => {
                if self.selection_state == SelectionState::Main {
                    return Ok(false);
                }
                self.set_index(0);
                self.selection_state = SelectionState::Main;
                self.episode_selection.series = None;
                self.episode_selection.episodes = None;
            }
            KeyCode::Up => {
                self.set_index(self.index(None).saturating_sub(1));
            }
            KeyCode::Down => {
                if self.index(None) < self.selection_options(None).len() - 1 {
                    self.set_index(self.index(None) + 1);
                }
            }
            KeyCode::PageUp => {
                self.set_index(
                    self.index(None)
                        .saturating_sub(self.main_selection.visible_height),
                );
            }
            KeyCode::PageDown => {
                self.set_index(
                    (self.index(None) + self.main_selection.visible_height)
                        .min(self.selection_options(None).len() - 1),
                );
            }
            KeyCode::Left => {
                if self.selection_state != SelectionState::Main {
                    return Ok(true);
                }

                match self.page {
                    Page::ContinueWatching => self.page = Page::AllSeries,
                    Page::NextUp => self.page = Page::ContinueWatching,
                    Page::LatestAdded => self.page = Page::NextUp,
                    Page::AllMovies => self.page = Page::LatestAdded,
                    Page::AllSeries => self.page = Page::AllMovies,
                    Page::All => {
                        self.page = {
                            if self.config.include_episodes {
                                Page::Episodes
                            } else {
                                Page::Series
                            }
                        }
                    }
                    Page::Movies => self.page = Page::All,
                    Page::Series => self.page = Page::Movies,
                    Page::Episodes => self.page = Page::Series,
                }
                self.search();
            }
            KeyCode::Right => {
                if self.selection_state != SelectionState::Main {
                    return Ok(true);
                }

                match self.page {
                    Page::ContinueWatching => self.page = Page::NextUp,
                    Page::NextUp => self.page = Page::LatestAdded,
                    Page::LatestAdded => self.page = Page::AllMovies,
                    Page::AllMovies => self.page = Page::AllSeries,
                    Page::AllSeries => self.page = Page::ContinueWatching,
                    Page::All => self.page = Page::Movies,
                    Page::Movies => self.page = Page::Series,
                    Page::Series => {
                        self.page = {
                            if self.config.include_episodes {
                                Page::Episodes
                            } else {
                                Page::All
                            }
                        }
                    }
                    Page::Episodes => self.page = Page::All,
                }

                self.search();
            }
            _ => {}
        }

        Ok(true)
    }

    async fn handle_action(&mut self) -> Result<bool> {
        match &self.current_action {
            Action::None => return Ok(false),
            Action::NowPlaying(item) => {
                self.jellyfin.play_media(item).await?;
            }
            Action::RefreshingCache => {
                self.jellyfin.refresh_cache().await?;
                if self.query.is_empty() {
                    self.search();
                }
            }
        }

        loop {
            if poll(Duration::from_millis(5))? {
                event::read()?;
            } else {
                break;
            }
        }

        self.current_action = Action::None;

        Ok(true)
    }

    fn draw_media_panel(
        &mut self,
        frame: &mut Frame,
        chunk: ratatui::prelude::Rect,
        item: Option<MediaItem>,
    ) {
        let item = match item {
            Some(item) => item,
            None => {
                let text = vec![Line::from("No item selected")];
                let widget = Paragraph::new(text)
                    .block(Block::default().title("Media Info").borders(Borders::ALL));
                return frame.render_widget(widget, chunk);
            }
        };

        let mut chunks: std::rc::Rc<[ratatui::prelude::Rect]> = std::rc::Rc::new([chunk]);

        let info_text;

        if item.type_ == "Episode" {
            chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(chunk);

            info_text = vec![
                Line::from(vec![Span::styled(
                    format!(
                        "S{:02}E{:02} - {}",
                        item.parent_index_number.unwrap_or(0),
                        item.index_number.unwrap_or(0),
                        item.name
                    ),
                    Style::default().add_modifier(Modifier::BOLD),
                )]),
                Line::from(""),
                Line::from(item.format_runtime()),
                Line::from(format!("Ends at {}", item.format_end_time())),
                Line::from(""),
                Line::from(vec![Span::styled(
                    "Episode Overview",
                    Style::default().add_modifier(Modifier::BOLD),
                )]),
            ];
        } else {
            info_text = vec![
                Line::from(vec![Span::styled(
                    &item.name,
                    Style::default().add_modifier(Modifier::BOLD),
                )]),
                Line::from(""),
                Line::from(format!(
                    "{}",
                    item.year
                        .map_or("Year unknown".to_string(), |y| y.to_string())
                )),
                Line::from(item.format_runtime()),
                Line::from(format!(
                    "IMDb: {}",
                    item.imdb_rating
                        .map_or("N/A".to_string(), |r| format!("{:.1}", r))
                )),
                Line::from(format!(
                    "Rotten Tomatoes: {}",
                    item.critic_rating
                        .map_or("N/A".to_string(), |r| format!("{}%", r))
                )),
                Line::from(format!("Ends at {}", item.format_end_time())),
                Line::from(""),
                Line::from(""),
                Line::from(vec![Span::styled(
                    "Overview",
                    Style::default().add_modifier(Modifier::BOLD),
                )]),
            ];
        }

        let overview = item.overview.as_deref().unwrap_or("No overview available");
        let max_width = chunks[0].width as usize - 4;
        let wrapped_overview: Vec<Line> = textwrap::wrap(overview, max_width)
            .into_iter()
            .map(|line| Line::from(line.to_string()))
            .collect();

        let mut all_lines = info_text;
        all_lines.extend(wrapped_overview);

        let info_widget = Paragraph::new(all_lines)
            .block(
                Block::default()
                    .title(format!("{} Info", item.type_))
                    .borders(Borders::ALL),
            )
            .wrap(ratatui::widgets::Wrap { trim: true });

        frame.render_widget(info_widget, *chunks.last().unwrap());

        if item.type_ != "Episode" {
            return;
        }

        let series_id = match &item.series_id {
            Some(series_id) => series_id,
            None => return,
        };

        let parent = match self.jellyfin.items.get(series_id) {
            Some(parent) => parent,
            None => return,
        };

        if parent.type_ != "Series" {
            return;
        }

        return self.draw_media_panel(frame, chunks[0], Some(parent.clone()));
    }

    fn draw_search_bar(&self, frame: &mut Frame, chunk: ratatui::prelude::Rect) {
        let search_block = Paragraph::new(self.query.as_str())
            .block(Block::default().title("Search").borders(Borders::ALL));
        frame.render_widget(search_block, chunk);
    }

    fn draw_main(
        &mut self,
        frame: &mut Frame,
        chunk: ratatui::prelude::Rect,
        state: SelectionState,
    ) {
        let mut lines = Vec::new();

        for (index, item) in enumerate(self.selection_options(Some(&state))) {
            let title = if let Some(year) = item.year {
                format!("  {} ({})", item.name, year)
            } else {
                format!("  {}", item.name)
            };

            let span = if index == self.index(Some(&state)) {
                vec![
                    Span::styled("> ".to_string(), Style::default().fg(Color::Yellow)),
                    Span::styled(
                        title.trim_start().to_string(),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]
            } else {
                vec![Span::raw(title.to_string())]
            };

            lines.push(Line::from(span));
        }

        let visible_height = chunk.height as usize - 2;

        let mut selection;

        match state {
            SelectionState::Main => {
                selection = self.main_selection.clone();
            }
            SelectionState::Episode => {
                selection = self.episode_selection.clone();
            }
        }

        selection.visible_height = visible_height;

        if selection.index < selection.scroll_position + 3 {
            selection.scroll_position = selection.index.saturating_sub(3);
        }

        if selection.index + 3 > (selection.scroll_position + visible_height) {
            selection.scroll_position = selection.index + 3 - visible_height;
        }

        let title = match state {
            SelectionState::Main => {
                self.main_selection = selection;
                let mut categories = if self.query.is_empty() {
                    vec![
                        ("Continue Watching", Page::ContinueWatching),
                        ("Next Up", Page::NextUp),
                        ("Latest Added", Page::LatestAdded),
                        ("Movies", Page::AllMovies),
                        ("Series", Page::AllSeries),
                    ]
                } else {
                    vec![
                        ("All", Page::All),
                        ("Movies", Page::Movies),
                        ("Series", Page::Series),
                    ]
                };

                if self.config.include_episodes && !self.query.is_empty() {
                    categories.push(("Episodes", Page::Episodes));
                }

                itertools::Itertools::intersperse(
                    categories.iter().map(|(name, page)| {
                        if *page == self.page {
                            Span::styled(
                                name.to_string(),
                                Style::default().add_modifier(Modifier::BOLD),
                            )
                        } else {
                            Span::raw(name.to_string())
                        }
                    }),
                    Span::raw(" "),
                )
                .collect::<Vec<_>>()
            }
            SelectionState::Episode => {
                self.episode_selection = selection;

                match &self.episode_selection.series {
                    Some(series) => vec![Span::raw(format!("{} Episodes", series.name))],
                    None => vec![Span::raw("No series selected")],
                }
            }
        };

        let lines = lines
            .iter()
            .skip(self.scroll_position(Some(&state)))
            .take(visible_height)
            .cloned()
            .collect::<Vec<_>>();

        let widget =
            Paragraph::new(lines).block(Block::default().title(title).borders(Borders::ALL));

        frame.render_widget(widget, chunk);
    }

    fn draw_action(&mut self, frame: &mut Frame, inner_area: Rect) {
        let popup_text;
        let title;

        match &self.current_action {
            Action::None => return,
            Action::NowPlaying(item) => {
                title = "Media Playing";
                popup_text = if item.type_ == "Episode" {
                    format!(
                        "Now Playing:\n\n{}\nS{:02}E{:02} - {}",
                        item.series_name.as_deref().unwrap_or(""),
                        item.parent_index_number.unwrap_or(0),
                        item.index_number.unwrap_or(0),
                        item.name
                    )
                } else {
                    format!("Now Playing:\n\n{}", item.name)
                };
            }
            Action::RefreshingCache => {
                title = "Refreshing";
                popup_text = "\nRefreshing cache and home page\nPlease wait...".to_string();
            }
        }

        let popup_width = 60.min(inner_area.width - 4);
        let popup_height = 6.min(inner_area.height - 4);

        let popup_area = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length((inner_area.width - popup_width) / 2),
                Constraint::Length(popup_width),
                Constraint::Min(0),
            ])
            .split(
                Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length((inner_area.height - popup_height) / 2),
                        Constraint::Length(popup_height),
                        Constraint::Min(0),
                    ])
                    .split(inner_area)[1],
            );

        let popup = Paragraph::new(popup_text)
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Red)),
            )
            .alignment(Alignment::Center)
            .wrap(ratatui::widgets::Wrap { trim: true });

        frame.render_widget(Clear, popup_area[1]);
        frame.render_widget(popup, popup_area[1]);
    }
}
