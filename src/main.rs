use std::{env, fs, io::Write, path::Path, sync::Arc};

use clap::Parser;
use dotenv_codegen::dotenv;
use futures::StreamExt;
use rspotify::{
    clients::mutex::Mutex,
    model::{Id, PlaylistId, TrackId},
    prelude::{OAuthClient, PlayableId},
    scopes, AuthCodeSpotify, Config, Credentials, OAuth,
};
use serde::{Deserialize, Serialize};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[arg(short, long, default_value = "default")]
    profile: String,
}

#[derive(Serialize, Deserialize)]
struct PlaymateConfig {
    playlist_id: Option<PlaylistId>,
    playlist_snapshot_id: Option<String>,
    playlist_track_cache: Option<Vec<TrackId>>,
}

impl PlaymateConfig {
    fn new() -> PlaymateConfig {
        PlaymateConfig {
            playlist_id: None,
            playlist_snapshot_id: None,
            playlist_track_cache: None,
        }
    }

    fn read_or_create_config_file(profile: &String) -> String {
        // Build the config string
        let appdata_dir = env::var_os("APPDATA").expect("No APPDATA environment variable?");
        let config_path = Path::new(&appdata_dir)
            .join("playmate")
            .join(profile) // This allows for custom profiles in the future
            .join("config.toml");

        // Check if the config file exists
        if fs::metadata(&config_path).is_err() {
            println!("Config file not found, creating new one");
            // Create the config file
            fs::create_dir_all(
                &config_path
                    .parent()
                    .expect("Error getting config path parent"),
            )
            .expect("Error creating config directory");
            fs::File::create(&config_path).expect("Error creating config file");
        }

        // Read the file
        fs::read_to_string(config_path).expect("Unable to read config file")
    }

    fn load(profile: &String) -> Self {
        let config_str = Self::read_or_create_config_file(profile);
        let config: PlaymateConfig = toml::from_str(&config_str).unwrap();
        config
    }

    fn save(&self, profile: &String) {
        let config_str = toml::to_string(&self).unwrap();
        let appdata_dir = env::var_os("APPDATA").expect("No APPDATA environment variable?");
        let config_path = Path::new(&appdata_dir)
            .join("playmate")
            .join(profile)
            .join("config.toml");
        fs::write(config_path, config_str).expect("Error writing config file");
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let mut config = PlaymateConfig::load(&cli.profile);
    let spotify = spotify_auth().await;
    if config.playlist_id.is_none() {
        config.playlist_id = Some(fetch_playlist_id(&spotify).await);
        config.save(&cli.profile);
    }

    // Get the currently playing track
    let current_track = spotify
        .current_user_playing_item()
        .await
        .expect("Error getting current track");
    if current_track.is_none() {
        println!("No track is playing");
        return;
    }

    // Get the id of the current track
    let current_track_id = current_track
        .unwrap()
        .item
        .expect("Unable to get currently playing item");
    let current_track_id = current_track_id.id();

    if current_track_id.is_none() {
        println!("The current track is local, so it cannot be added to the playlist");
        return;
    }
    // Remove the current track from the playlist
    let _snapshot_id = spotify
        .playlist_remove_all_occurrences_of_items(
            &config.playlist_id.clone().unwrap(),
            current_track_id,
            None,
        )
        .await
        .expect("Error removing current track from playlist")
        .snapshot_id;
    // Add the current track to the playlist
    let _snapshot_id = spotify
        .playlist_add_items(&config.playlist_id.unwrap(), current_track_id, None)
        .await
        .expect("Error adding current track to playlist")
        .snapshot_id;
}

async fn fetch_playlist_id(spotify: &AuthCodeSpotify) -> PlaylistId {
    let playlists = spotify.current_user_playlists().collect::<Vec<_>>().await;

    // Print the playlist and prompt the user to select one
    loop {
        println!("Select a playlist");
        for (count, p) in playlists.iter().enumerate() {
            println!(
                "{:>4}: {}",
                count + 1,
                p.as_ref().expect("Error iterating over playlists").name
            );
        }
        print!(
            "Select which playlist to use by typing the playlist's number and pressing enter:\n> "
        );
        std::io::stdout()
            .flush()
            .expect("Error flushing selection instructions");
        let mut selection = String::new();
        std::io::stdin()
            .read_line(&mut selection)
            .expect("Error reading selection");
        let selection = selection.trim();
        // Make sure the selection is valid
        if selection.parse::<usize>().is_err() {
            println!("Invalid selection\n");
            continue;
        }
        let selection = selection.parse::<usize>().unwrap();
        if selection < 1 || selection > playlists.len() {
            println!("Invalid selection\n");
            continue;
        }
        return playlists[selection - 1]
            .as_ref()
            .expect("Error selecting playlist")
            .id
            .clone();
    }
}

async fn spotify_auth() -> AuthCodeSpotify {
    // let creds = Credentials::from_env().expect("Failed to get app credentials");
    let creds = Credentials::new(
        dotenv!("RSPOTIFY_CLIENT_ID"),
        dotenv!("RSPOTIFY_CLIENT_SECRET"),
    );
    let oauth = OAuth {
        redirect_uri: "http://localhost:8888/callback".to_string(),
        scopes: scopes!(
            "user-read-currently-playing",
            "user-read-playback-state",
            "playlist-read-private",
            "playlist-modify-private",
            "user-library-modify",
            "user-library-read"
        ),
        ..Default::default()
    };
    let appdata_dir = env::var_os("APPDATA").expect("No APPDATA environment variable?");
    let config = Config {
        token_cached: true,
        token_refreshing: true,
        cache_path: Path::new(&appdata_dir)
            .join("playmate")
            .join("token_cache.json"),
        // pagination_chunks: 100,
        ..Default::default()
    };
    let mut spotify = AuthCodeSpotify::with_config(creds, oauth, config);
    if !spotify.config.cache_path.exists() {
        println!(
            "A browser window will open to prompt you to log in to Spotify. \
        Once you have logged in, it will redirect you to a page that will show you an error. \
        This is expected. Copy the URL of the page and paste it into the terminal.\
        \nPress enter to continue. "
        );
        std::io::stdin()
            .read_line(&mut String::new())
            .expect("Error reading enter");
        let url = spotify.get_authorize_url(false).unwrap();
        spotify
            .prompt_for_token(&url)
            .await
            .expect("Couldn't authenticate successfully");
    }
    spotify.token = Arc::new(Mutex::new(spotify.read_token_cache(true).await.unwrap()));
    spotify
}
