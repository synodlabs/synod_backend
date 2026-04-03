use serde::Deserialize;

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
            .add_source(config::Environment::with_prefix("SYNOD").separator("__"))
            .build()?;

        let settings: Settings = config.try_deserialize()?;
        Ok(settings)
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
