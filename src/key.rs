use std::{path::Path, sync::LazyLock};

use anyhow::{Context, Result, anyhow};
use iroh::SecretKey;
use tokio::{
    fs::{self, File},
    io::AsyncWriteExt,
};

use tracing::info;
use xdg::BaseDirectories;

static DIRS: LazyLock<BaseDirectories> = LazyLock::new(|| BaseDirectories::with_prefix("n2p"));

pub fn generate_secret_key() -> SecretKey {
    SecretKey::generate(&mut rand::rngs::OsRng)
}

pub async fn read_key_from_file(path: impl AsRef<Path>) -> Result<SecretKey> {
    let key_bytes = fs::read(&path)
        .await
        .with_context(|| format!("could not read key file {}", path.as_ref().display()))?;

    let data = <[u8; 32]>::try_from(key_bytes).map_err(|bytes| {
        anyhow!(
            "key file has unexpected size of {}, expected 32",
            bytes.len(),
        )
    })?;

    let key = SecretKey::from_bytes(&data);
    Ok(key)
}

pub async fn read_user_key() -> Result<Option<SecretKey>> {
    let Some(key_file) = DIRS.find_state_file("secret.key") else {
        return Ok(None);
    };

    Some(read_key_from_file(key_file).await).transpose()
}

pub async fn write_user_key(key: &SecretKey) -> Result<()> {
    let key_file = DIRS
        .place_state_file("secret.key")
        .context("could not prepare state dir")?;

    info!(?key_file, "writing secret key to default dir");

    let mut file = File::options()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&key_file)
        .await
        .context("could not open key file")?;

    file.write_all(&key.to_bytes()).await?;
    Ok(())
}
