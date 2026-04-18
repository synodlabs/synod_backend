use serde::Deserialize;
use std::env;

#[derive(Debug, Deserialize, Clone)]
pub struct Settings {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub redis: RedisConfig,
    pub stellar: StellarConfig,
    pub auth: AuthConfig,
    pub walletconnect: WalletConnectConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RedisConfig {
    pub url: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct StellarConfig {
    pub network: String,
    pub network_passphrase: String, // Added this
    pub horizon_url: String,
    pub coordinator_pubkey: String,
    pub coordinator_secret_key: String,
    pub coordinator_secret_key_path: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AuthConfig {
    pub jwt_secret: String,
    pub jwt_expiry_hours: u64,
    pub bcrypt_cost: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WalletConnectConfig {
    pub project_id: String,
    pub relay_url: String,
}

impl Settings {
    pub fn load() -> anyhow::Result<Self> {
        let config = config::Config::builder()
            .add_source(config::File::with_name("config/default").required(false))
            .add_source(config::File::with_name("synod-coordinator/config/default").required(false))
            .add_source(config::Environment::with_prefix("SYNOD").separator("__"))
            .build()?;

        let mut settings: Settings = config.try_deserialize()?;
        settings.apply_runtime_env_overrides();
        Ok(settings)
    }

    fn apply_runtime_env_overrides(&mut self) {
        if let Ok(port) = env::var("PORT") {
            if let Ok(parsed) = port.parse::<u16>() {
                self.server.port = parsed;
            }
        }

        if let Ok(host) = env::var("HOST") {
            if !host.trim().is_empty() {
                self.server.host = host;
            }
        }

        if let Ok(url) = env::var("DATABASE_URL") {
            if !url.trim().is_empty() {
                self.database.url = url;
            }
        }

        if let Ok(url) = env::var("REDIS_URL") {
            if !url.trim().is_empty() {
                self.redis.url = url;
            }
        }

        if let Ok(network) = env::var("STELLAR_NETWORK") {
            if !network.trim().is_empty() {
                self.stellar.network = network;
            }
        }

        if let Ok(horizon_url) = env::var("HORIZON_URL") {
            if !horizon_url.trim().is_empty() {
                self.stellar.horizon_url = horizon_url;
            }
        }

        if let Ok(network_passphrase) = env::var("SYNOD_STELLAR__NETWORK_PASSPHRASE") {
            if !network_passphrase.trim().is_empty() {
                self.stellar.network_passphrase = network_passphrase;
            }
        }

        if let Ok(coordinator_pubkey) = env::var("SYNOD_STELLAR__COORDINATOR_PUBKEY") {
            if !coordinator_pubkey.trim().is_empty() {
                self.stellar.coordinator_pubkey = coordinator_pubkey;
            }
        }

        if let Ok(coordinator_secret_key) = env::var("SYNOD_STELLAR__COORDINATOR_SECRET_KEY") {
            if !coordinator_secret_key.trim().is_empty() {
                self.stellar.coordinator_secret_key = coordinator_secret_key;
            }
        }

        if let Ok(jwt_secret) = env::var("JWT_SECRET") {
            if !jwt_secret.trim().is_empty() {
                self.auth.jwt_secret = jwt_secret;
            }
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 8080,
        }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: "postgres://postgres:postgres@localhost:5432/synod_db".to_string(),
            max_connections: 20,
        }
    }
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            url: "redis://localhost:6379".to_string(),
        }
    }
}

