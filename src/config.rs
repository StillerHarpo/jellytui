use std::io;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;
use directories::BaseDirs;
use rpassword::read_password;
use serde::{Deserialize, Serialize};
use toml::{from_str, to_string};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    #[serde(default)]
    pub accept_self_signed: bool,
    pub server_url: String,
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub is_new: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            accept_self_signed: false,
            server_url: String::new(),
            username: String::new(),
            password: String::new(),
            is_new: false,
        }
    }
}

impl Config {
    pub fn config_path(base_path: Option<&Path>) -> Option<PathBuf> {
        base_path.map(|p| p.join("config.toml")).or(BaseDirs::new()
            .map(|base_dirs| base_dirs.config_dir().join("jellytui").join("config.toml")))
    }

    pub fn load(base_path: Option<&Path>) -> Result<Self> {
        let config_path = Self::config_path(base_path)
            .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?;

        if !config_path.exists() {
            let config = Self::create_initial_config()?;
            let toml = to_string(&config)?.replace("\nis_new = true", "");
            std::fs::create_dir_all(config_path.parent().unwrap())?;
            std::fs::write(&config_path, toml)?;

            return Ok(config);
        }

        let mut contents = std::fs::read_to_string(config_path)?;
        contents.push_str("\nis_new = false");
        let config: Config = from_str(&contents)?;

        Ok(config)
    }

    pub fn delete(base_path: Option<&Path>) -> Result<()> {
        let config_path = Self::config_path(base_path)
            .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?;

        if config_path.exists() {
            std::fs::remove_file(config_path)?;
        }

        Ok(())
    }

    fn create_initial_config() -> Result<Self> {
        print!("\x1B[2J\x1B[1;1H");
        println!("Config file not found");

        print!("Does your server have a self-signed https certificate? [y/n]\n> ");
        io::stdout().flush()?;
        let mut accept_self_signed = String::new();
        io::stdin().read_line(&mut accept_self_signed)?;
        let accept_self_signed = accept_self_signed.trim().to_string().to_lowercase() == "y";

        print!("Please enter the URL of your Jellyfin server. Example: http://foobar.baz:8096/jf\n\
               (note: unless specified, ports will be the protocol's defaults, i.e. 80 for HTTP and 443 for HTTPS)\n> ");
        io::stdout().flush()?;
        let mut server_url = String::new();
        io::stdin().read_line(&mut server_url)?;
        let server_url = server_url.trim().to_string();

        print!("Please enter your username\n> ");
        io::stdout().flush()?;
        let mut username = String::new();
        io::stdin().read_line(&mut username)?;
        let username = username.trim().to_string();

        print!("Please enter your password\n> ");
        io::stdout().flush()?;
        let password = read_password()?;

        print!("\x1B[2J\x1B[1;1H");
        io::stdout().flush()?;

        Ok(Config {
            accept_self_signed,
            server_url,
            username,
            password,
            is_new: true,
        })
    }
}
