use std::io;
use std::io::Write;
use std::path::PathBuf;

use anyhow::Result;
use directories::BaseDirs;
use rpassword::read_password;
use serde::{Deserialize, Serialize};
use toml::{from_str, to_string};

#[derive(Debug, Clone)]
pub struct ConfigWithPassword {
    pub config: Config,
    pub password: String,
    pub is_new: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    #[serde(default)]
    pub accept_self_signed: bool,
    pub server_url: String,
    pub username: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            accept_self_signed: false,
            server_url: String::new(),
            username: String::new(),
        }
    }
}

impl Config {
    pub fn config_path() -> Option<PathBuf> {
        BaseDirs::new().map(|base_dirs| base_dirs.config_dir().join("jellytui").join("config.toml"))
    }

    pub fn load() -> Result<(Self, bool)> {
        let config_path = Self::config_path()
            .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?;

        if !config_path.exists() {
            let config = Self::create_initial_config()?;
            let toml = to_string(&config)?;
            std::fs::create_dir_all(config_path.parent().unwrap())?;
            std::fs::write(&config_path, toml)?;

            return Ok((config, true));
        }

        let mut contents = std::fs::read_to_string(config_path)?;
        contents.push_str("\nis_new = false");
        let config: Config = from_str(&contents)?;

        Ok((config, false))
    }

    pub fn delete() -> Result<()> {
        let config_path = Self::config_path()
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

        Ok(Config {
            accept_self_signed,
            server_url,
            username,
        })
    }
}

impl ConfigWithPassword {
    pub fn password_path() -> Option<PathBuf> {
        BaseDirs::new().map(|base_dirs| base_dirs.config_dir().join("jellytui").join(".password"))
    }

    pub fn load() -> Result<Self> {
        let (config, is_new) = Config::load()?;
        let password_path = Self::password_path()
            .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?;
        if !password_path.exists() {
            std::fs::create_dir_all(password_path.parent().unwrap())?;
            let password = Self::create_initial_password()?;
            std::fs::write(&password_path, &password)?;
            return Ok(ConfigWithPassword {
                config,
                password,
                is_new: true,
            });
        }

        let password = std::fs::read_to_string(password_path)?;

        Ok(ConfigWithPassword {
            config,
            password,
            is_new,
        })
    }

    pub fn delete() -> Result<()> {
        Config::delete()?;

        let password_path = Self::password_path()
            .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?;

        if password_path.exists() {
            std::fs::remove_file(password_path)?;
        }

        Ok(())
    }

    fn create_initial_password() -> Result<String> {
        print!("Please enter your password\n> ");
        io::stdout().flush()?;
        let password = read_password()?;

        print!("\x1B[2J\x1B[1;1H");
        io::stdout().flush()?;

        Ok(password)
    }
}
