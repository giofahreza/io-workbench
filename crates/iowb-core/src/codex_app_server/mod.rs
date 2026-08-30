use std::{ffi::OsString, path::PathBuf, process::Stdio, sync::Arc, time::Duration};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::{Mutex, mpsc, oneshot},
    time::{Instant, Sleep, timeout},
};
use tracing::warn;

use crate::{CoreError, Result, augmented_user_path};

// The app-server protocol is kept in focused files while sharing this module's
// private types and helpers.
include!("types.rs");
include!("client.rs");
include!("live_turn.rs");
include!("protocol.rs");
include!("thread.rs");

#[cfg(test)]
mod tests {
    include!("tests.rs");
}
