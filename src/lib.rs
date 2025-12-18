pub mod client;
pub mod key;
pub mod server;

use std::sync::LazyLock;

pub use iroh;
use xdg::BaseDirectories;

static DIRS: LazyLock<BaseDirectories> = LazyLock::new(|| BaseDirectories::with_prefix("n2p"));
