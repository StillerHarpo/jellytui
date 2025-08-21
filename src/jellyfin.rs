use ratatui::widgets::Paragraph;
use ratatui::{layout::Rect, DefaultTerminal, Frame};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use directories::BaseDirs;
use hostname;
use reqwest::blocking::{Client, RequestBuilder, Response};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::config::Config;

#[derive(Debug, Deserialize, Clone)]
struct AuthResponse {
    #[serde(rename = "AccessToken")]
    access_token: String,
    #[serde(rename = "User")]
    user: JellyfinUser,
}

#[derive(Debug, Deserialize, Clone)]
struct JellyfinUser {
    #[serde(rename = "Id")]
    id: String,
    #[serde(rename = "Configuration")]
    config: JellyfinUserConfig,
}

#[derive(Debug, Deserialize, Clone)]
struct JellyfinUserConfig {
    #[serde(rename = "AudioLanguagePreference")]
    audio_language_preference: Option<String>,
    #[serde(rename = "PlayDefaultAudioTrack")]
    play_default_audio_track: bool,
    #[serde(rename = "SubtitleLanguagePreference")]
    subtitle_language_preference: String,
}

#[derive(Debug, Deserialize)]
struct JellyfinItemsResponse {
    #[serde(rename = "Items")]
    items: Vec<MediaItem>,
}

#[derive(Debug, Deserialize)]
struct PlaybackInfo {
    #[serde(rename = "MediaSources")]
    media_sources: Vec<MediaSource>,
}

#[derive(Debug, Deserialize)]
struct MediaSource {
    #[serde(rename = "RunTimeTicks")]
    runtime_ticks: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MediaItem {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Type")]
    pub type_: String,
    #[serde(rename = "Path")]
    pub path: Option<String>,
    #[serde(rename = "CollectionType")]
    pub collection_type: Option<String>,
    #[serde(rename = "ProductionYear")]
    pub year: Option<i32>,
    #[serde(rename = "Overview")]
    pub overview: Option<String>,
    #[serde(rename = "CommunityRating")]
    pub imdb_rating: Option<f32>,
    #[serde(rename = "CriticRating")]
    pub critic_rating: Option<i32>,
    #[serde(rename = "RunTimeTicks")]
    pub runtime_ticks: Option<i64>,
    #[serde(rename = "SeriesId")]
    pub series_id: Option<String>,
    #[serde(rename = "SeriesName")]
    pub series_name: Option<String>,
    #[serde(rename = "ParentIndexNumber")]
    pub parent_index_number: Option<i64>,
    #[serde(rename = "IndexNumber")]
    pub index_number: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct Jellyfin {
    pub items: HashMap<String, MediaItem>,
    pub continue_watching: Vec<MediaItem>,
    pub next_up: Vec<MediaItem>,
    pub latest_added: Vec<MediaItem>,
    client: Client,
    config: Config,
    auth: Option<AuthResponse>,
    mpv_processes: Arc<Mutex<Vec<Child>>>,
    cache_path: PathBuf,
}

impl MediaItem {
    pub fn format_runtime(&self) -> String {
        let Some(ticks) = self.runtime_ticks else {
            return "Unknown runtime".to_string();
        };

        let total_minutes = (ticks / (10_000_000 * 60)) as i64;
        let hours = total_minutes / 60;
        let minutes = total_minutes % 60;

        if hours > 0 {
            format!("{}h {}m", hours, minutes)
        } else {
            format!("{}m", minutes)
        }
    }

    pub fn format_end_time(&self) -> String {
        let Some(ticks) = self.runtime_ticks else {
            return "Unknown runtime".to_string();
        };

        (chrono::Local::now() + chrono::Duration::seconds(ticks / 10_000_000))
            .format("%H:%M")
            .to_string()
    }
}

impl Jellyfin {
    pub fn new(
        base_path: Option<&Path>,
        config: Config,
        opt_terminal: &mut Option<&mut DefaultTerminal>,
        render_outer: fn(&mut Frame) -> Rect,
    ) -> Result<Self> {
        // cache directory init
        let cache_path = base_path
            .map(|p| p.join("cache.json"))
            .or(BaseDirs::new().map(|base_dirs| {
                base_dirs
                    .data_local_dir()
                    .join("jellytui")
                    .join("cache.json")
            }))
            .unwrap();

        if let Some(parent) = cache_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut jellyfin = Jellyfin {
            items: HashMap::new(),
            continue_watching: Vec::new(),
            next_up: Vec::new(),
            latest_added: Vec::new(),
            client: Client::builder()
                .danger_accept_invalid_certs(config.accept_self_signed)
                .build()?,
            config,
            auth: None,
            mpv_processes: Arc::new(Mutex::new(Vec::new())),
            cache_path,
        };
        macro_rules! log {
            ($txt:expr) => {
                match opt_terminal {
                    Some(terminal) => {
                        terminal.draw(|frame| {
                            let inner_area = render_outer(frame);
                            frame.render_widget(Paragraph::new($txt), inner_area);
                        })?;
                    }
                    None => {
                        println!($txt);
                    }
                }
            };
        }
        log!("Authenticating...");

        match jellyfin.authenticate() {
            Ok(_) => {}
            Err(e) => {
                eprintln!("Failed to authenticate: {}", e);

                if !jellyfin.config.is_new {
                    log!("Would you like to delete the current configuration? (y/n):\n> ");

                    std::io::stdout().flush()?;
                    let mut delete = String::new();
                    std::io::stdin().read_line(&mut delete)?;

                    if delete.trim().to_lowercase() != "y" {
                        std::process::exit(1);
                    }

                    log!("Deleting configuration... run again to reconfigure");
                }
                Config::delete(base_path)?;
                std::process::exit(1);
            }
        }
        log!("Fetching media... this may take a while on the first run");
        jellyfin.fetch_all_media()?;
        log!("Fetching home sections...");
        jellyfin.fetch_home_sections()?;

        Ok(jellyfin)
    }

    fn request(&mut self, request: RequestBuilder) -> Result<Response> {
        let response = request
            .try_clone()
            .expect("Failed to clone request")
            .header(
                "X-MediaBrowser-Token",
                &self.auth.as_ref().unwrap().access_token,
            )
            .send()?;

        if response.status() != StatusCode::UNAUTHORIZED {
            return Ok(response);
        }

        self.authenticate()?;

        Ok(request
            .header(
                "X-MediaBrowser-Token",
                &self.auth.as_ref().unwrap().access_token,
            )
            .send()?)
    }

    fn authenticate(&mut self) -> Result<()> {
        let device_name = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown-device".to_string());

        let auth_request = serde_json::json!({
            "Username": self.config.username,
            "Pw": self.config.password
        });

        let response = self.client
            .post(format!("{}/Users/AuthenticateByName", self.config.server_url))
            .header("X-Emby-Authorization", format!(
                "MediaBrowser Client=\"jellytui\", Device=\"{}\", DeviceId=\"tui\", Version=\"1.0.0\"",
                device_name
            ))
            .json(&auth_request)
            .send()?;

        match response.status() {
            StatusCode::UNAUTHORIZED => {
                return Err(anyhow::anyhow!("401: Invalid username or password"));
            }
            StatusCode::FORBIDDEN => {
                return Err(anyhow::anyhow!("403: Access to server denied"));
            }
            _ => {}
        }

        self.auth = Some(response.json::<AuthResponse>()?);

        Ok(())
    }

    fn fetch_all_media(&mut self) -> Result<()> {
        if let Ok(cached) = fs::read_to_string(&self.cache_path) {
            if let Ok(items) = serde_json::from_str::<HashMap<String, MediaItem>>(&cached) {
                self.items = items;
                return Ok(());
            }
        }

        self.items = self
            .request(
                self.client
                    .get(format!(
                        "{}/Users/{}/Items",
                        self.config.server_url,
                        &self.auth.as_ref().unwrap().user.id
                    ))
                    .query(&[
                        ("Recursive", "true"),
                        (
                            "Fields",
                            "Path,Overview,CommunityRating,CriticRating,RunTimeTicks",
                        ),
                        ("IncludeItemTypes", "Movie,Series,Episode"),
                        ("SortBy", "SortName"),
                        ("SortOrder", "Ascending"),
                    ]),
            )?
            .json::<JellyfinItemsResponse>()?
            .items
            .into_iter()
            .map(|item| (item.id.clone(), item))
            .collect();

        fs::write(&self.cache_path, serde_json::to_string(&self.items)?)?;

        Ok(())
    }

    fn fetch_home_sections(&mut self) -> Result<()> {
        let user_id = self.auth.clone().unwrap().user.id;

        self.continue_watching = self
            .request(
                self.client
                    .get(format!(
                        "{}/Users/{}/Items/Resume",
                        self.config.server_url, user_id
                    ))
                    .query(&[
                        ("Limit", "12"),
                        (
                            "Fields",
                            "Path,Overview,CommunityRating,CriticRating,RunTimeTicks",
                        ),
                    ]),
            )?
            .json::<JellyfinItemsResponse>()?
            .items;

        self.next_up = self
            .request(
                self.client
                    .get(format!("{}/Shows/NextUp", self.config.server_url))
                    .query(&[
                        ("UserId", user_id.as_str()),
                        ("Limit", "12"),
                        (
                            "Fields",
                            "Path,Overview,CommunityRating,CriticRating,RunTimeTicks",
                        ),
                    ]),
            )?
            .json::<JellyfinItemsResponse>()?
            .items;

        self.latest_added = self
            .request(
                self.client
                    .get(format!(
                        "{}/Users/{}/Items",
                        self.config.server_url, user_id
                    ))
                    .query(&[
                        ("Limit", "12"),
                        (
                            "Fields",
                            "Path,Overview,CommunityRating,CriticRating,RunTimeTicks",
                        ),
                        ("IncludeItemTypes", "Movie,Series"),
                        ("SortBy", "DateCreated,SortName"),
                        ("SortOrder", "Descending"),
                        ("Recursive", "true"),
                    ]),
            )?
            .json::<JellyfinItemsResponse>()?
            .items;

        Ok(())
    }

    pub fn get_episodes_from_series(&self, series_id: &str) -> Vec<MediaItem> {
        let mut episodes: Vec<_> = self
            .items
            .values()
            .filter(|item| item.series_id.as_deref() == Some(series_id))
            .cloned()
            .collect();

        episodes.sort_by(|a, b| {
            (
                a.parent_index_number.unwrap_or(0),
                a.index_number.unwrap_or(0),
            )
                .cmp(&(
                    b.parent_index_number.unwrap_or(0),
                    b.index_number.unwrap_or(0),
                ))
        });

        episodes
    }

    pub fn play_media(&mut self, item: &MediaItem) -> Result<Option<MediaItem>> {
        let playback_info = self
            .request(
                self.client
                    .post(format!(
                        "{}/Items/{}/PlaybackInfo",
                        self.config.server_url, item.id
                    ))
                    .json(&serde_json::json!({
                        "DeviceProfile": {
                            "MaxStreamingBitrate": 140000000,
                            "DirectPlayProfiles": [
                                {
                                    "Container": "mkv,mp4,avi",
                                    "Type": "Video",
                                    "VideoCodec": "h264,hevc,mpeg4,mpeg2video",
                                    "AudioCodec": "aac,mp3,ac3,eac3,flac,vorbis,opus"
                                }
                            ],
                            "TranscodingProfiles": []
                        }
                    })),
            )?
            .json::<PlaybackInfo>()?;

        let source = playback_info
            .media_sources
            .first()
            .ok_or_else(|| anyhow::anyhow!("No media source available"))?;

        let position_url = format!("{}/UserItems/{}/UserData", self.config.server_url, item.id);

        let position_ticks = self
            .request(self.client.get(&position_url))?
            .json::<serde_json::Value>()?
            .get("PlaybackPositionTicks")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        let position_seconds = position_ticks / 10_000_000;

        let runtime_seconds = source.runtime_ticks / 10_000_000;

        let auth = self.auth.clone().unwrap();

        let stream_url = format!(
            "{}/Videos/{}/stream?static=true&mediaSourceId={}&tag={}",
            self.config.server_url, item.id, item.id, auth.access_token
        );

        let title = if item.type_ == "Episode" {
            format!(
                "  {} - S{:02}E{:02} - {}",
                item.series_name.as_deref().unwrap_or("Unknown Series"),
                item.parent_index_number.unwrap_or(0),
                item.index_number.unwrap_or(0),
                item.name
            )
        } else if let Some(year) = item.year {
            format!("  {} ({})", item.name, year)
        } else {
            format!("  {}", item.name)
        };

        let socket_path = format!("/tmp/mpv-socket-{}", item.id);

        let mut command = Command::new("mpv");
        command
            .arg(stream_url)
            .arg("--no-cache-pause")
            .arg(format!("--demuxer-lavf-probe-info=yes"))
            .arg(format!("--demuxer-lavf-analyzeduration=10"))
            .arg(format!("--length={}", runtime_seconds))
            .arg(format!("--force-media-title={}", title))
            .arg(format!(
                "--http-header-fields=X-MediaBrowser-Token: {}",
                auth.access_token
            ))
            .arg(format!("--input-ipc-server={}", socket_path));

        if !auth.user.config.play_default_audio_track
            && auth.user.config.audio_language_preference.is_some()
        {
            command.arg(format!(
                "--alang={}",
                auth.user.config.audio_language_preference.unwrap()
            ));
        }

        if auth.user.config.subtitle_language_preference == "none" {
            command.arg("--no-sub");
        } else {
            command.arg(format!(
                "--slang={}",
                auth.user.config.subtitle_language_preference
            ));

            command.arg("--sub-auto=fuzzy");
        }

        if position_seconds > 0 {
            command.arg(format!("--start={}", position_seconds));
        }

        let child = command
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;

        self.mpv_processes.lock().unwrap().push(child);

        // wait for mpv to start
        std::thread::sleep(Duration::from_secs(2));

        let next = self.monitor_playback(item, &socket_path);

        std::fs::remove_file(socket_path)?;

        next
    }

    fn monitor_playback(
        &mut self,
        item: &MediaItem,
        socket_path: &String,
    ) -> Result<Option<MediaItem>> {
        let mut last_position = 0i64;
        let mut last_update = std::time::Instant::now();

        let timeout = Duration::from_secs(10);
        let retry_delay = Duration::from_millis(50);

        let mut socket = loop {
            match UnixStream::connect(&socket_path) {
                Ok(socket) => break socket,
                Err(_) => {
                    if last_update.elapsed() >= timeout {
                        return Ok(None);
                    }
                    std::thread::sleep(retry_delay);
                }
            }
        };

        if let Err(e) = socket.write_all(
            b"{\"command\":[\"observe_property\",1,\"playback-time\"]}\n\
            {\"command\":[\"observe_property\",2,\"pause\"]}\n\
            {\"command\":[\"observe_property\",3,\"eof-reached\"]}\n",
        ) {
            eprintln!("Failed to write to socket: {}", e);
            return Ok(None);
        }

        let mut buffer = [0u8; 1024];
        while let Ok(n) = socket.read(&mut buffer) {
            if n == 0 {
                break;
            }

            let Ok(response) = serde_json::from_slice::<serde_json::Value>(&buffer[..n]) else {
                continue;
            };

            let Some(event) = response.get("event") else {
                continue;
            };

            match event.as_str().unwrap() {
                "property-change" => {
                    let Some(name) = response.get("name") else {
                        continue;
                    };

                    match name.as_str().unwrap() {
                        "pause" => {
                            let Some(data) = response.get("data") else {
                                continue;
                            };

                            let Some(paused) = data.as_bool() else {
                                continue;
                            };

                            if let Err(e) = self.request(
                                self.client
                                    .post(format!(
                                        "{}/Sessions/Playing/Progress",
                                        self.config.server_url
                                    ))
                                    .json(&serde_json::json!({
                                        "ItemId": item.id,
                                        "PositionTicks": last_position,
                                        "IsPaused": paused
                                    })),
                            ) {
                                eprintln!("Failed to update pause state: {}", e);
                            }
                        }
                        "playback-time" => {
                            let Some(data) = response.get("data") else {
                                continue;
                            };

                            let Some(position) = data.as_f64() else {
                                continue;
                            };

                            let position_ticks = (position * 10_000_000.0) as i64;

                            if (position_ticks - last_position).abs() < 50_000_000
                                || last_update.elapsed() < Duration::from_secs(10)
                            {
                                continue;
                            }

                            if let Err(e) = self.request(
                                self.client
                                    .post(format!(
                                        "{}/Sessions/Playing/Progress",
                                        self.config.server_url
                                    ))
                                    .json(&serde_json::json!({
                                        "ItemId": item.id,
                                        "PositionTicks": position_ticks
                                    })),
                            ) {
                                eprintln!("Failed to update progress: {}", e);
                            }

                            last_position = position_ticks;
                            last_update = std::time::Instant::now();
                        }
                        _ => {}
                    }
                }
                "end-file" => {
                    if response.get("reason") == Some(&serde_json::Value::String("eof".to_string()))
                    {
                        return Ok(self
                            .get_episodes_from_series(item.series_id.as_deref().unwrap())
                            .iter()
                            .find(|ep| {
                                ep.index_number == item.index_number.map(|i| i + 1)
                                    || ep.parent_index_number
                                        == item.parent_index_number.map(|i| i + 1)
                                        && ep.index_number == Some(1)
                            })
                            .cloned());
                    }
                }
                _ => {}
            }
        }

        if let Err(e) = self.request(
            self.client
                .post(format!(
                    "{}/Sessions/Playing/Stopped",
                    self.config.server_url
                ))
                .json(&serde_json::json!({
                    "ItemId": item.id,
                    "PositionTicks": last_position
                })),
        ) {
            eprintln!("Failed to update progress: {}", e);
        }

        return Ok(None);
    }

    pub fn refresh_cache(&mut self) -> Result<()> {
        fs::remove_file(&self.cache_path)?;

        self.fetch_all_media()?;
        self.fetch_home_sections()?;

        Ok(())
    }

    pub fn cleanup(&self) -> Result<()> {
        let Ok(mut processes) = self.mpv_processes.lock() else {
            return Ok(());
        };

        for process in processes.iter_mut() {
            process.kill()?;
            process.wait()?;
        }

        processes.clear();

        Ok(())
    }
}
