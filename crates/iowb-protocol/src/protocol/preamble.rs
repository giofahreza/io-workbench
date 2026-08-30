use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const PRODUCT_NAME: &str = "io-workbench";
pub const SHORT_ALIAS: &str = "iowb";
pub const CONFIG_DIR_NAME: &str = ".io-workbench";
pub const DATABASE_FILE_NAME: &str = "io-workbench.db";
pub const ENV_PREFIX: &str = "IO_WORKBENCH_";

pub const WS_COMMAND_CHANNEL_CAPACITY: usize = 128;
pub const WS_EVENT_CHANNEL_CAPACITY: usize = 512;
pub const AUTO_SESSION_TITLE_MAX_CHARS: usize = 100;
