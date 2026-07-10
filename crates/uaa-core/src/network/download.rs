// file: crates/uaa-core/src/network/download.rs
// version: 1.0.2
// guid: u1v2w3x4-y5z6-7890-1234-567890uvwxyz

//! Network download utilities

use crate::Result;
use futures::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::Path;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tracing::{debug, info};

#[cfg(test)]
use std::sync::{Mutex, OnceLock};

#[cfg(test)]
#[derive(Default)]
struct MockResponses {
    download_with_progress: Option<Result<()>>,
    get_file_size: Option<Result<Option<u64>>>,
    verify_url: Option<Result<bool>>,
    download: Option<Result<()>>,
}

#[cfg(test)]
static MOCK_RESPONSES: OnceLock<Mutex<MockResponses>> = OnceLock::new();

#[cfg(test)]
fn mock_storage() -> &'static Mutex<MockResponses> {
    MOCK_RESPONSES.get_or_init(|| Mutex::new(MockResponses::default()))
}

#[cfg(test)]
fn take_mock_download_with_progress() -> Option<Result<()>> {
    mock_storage().lock().unwrap().download_with_progress.take()
}

#[cfg(test)]
fn take_mock_download() -> Option<Result<()>> {
    mock_storage().lock().unwrap().download.take()
}

#[cfg(test)]
fn take_mock_get_file_size() -> Option<Result<Option<u64>>> {
    mock_storage().lock().unwrap().get_file_size.take()
}

#[cfg(test)]
fn take_mock_verify_url() -> Option<Result<bool>> {
    mock_storage().lock().unwrap().verify_url.take()
}

#[cfg(test)]
pub(crate) fn set_mock_download_with_progress(result: Result<()>) {
    mock_storage().lock().unwrap().download_with_progress = Some(result);
}

#[cfg(test)]
pub(crate) fn set_mock_get_file_size(result: Result<Option<u64>>) {
    mock_storage().lock().unwrap().get_file_size = Some(result);
}

#[cfg(test)]
pub(crate) fn set_mock_verify_url(result: Result<bool>) {
    mock_storage().lock().unwrap().verify_url = Some(result);
}

/// Network downloader with progress tracking
pub struct NetworkDownloader {
    client: Option<reqwest::Client>,
}

impl NetworkDownloader {
    /// Create a new network downloader
    pub fn new() -> Self {
        #[cfg(test)]
        {
            Self { client: None }
        }

        #[cfg(not(test))]
        {
            Self {
                client: Some(reqwest::Client::new()),
            }
        }
    }

    /// Download file with progress bar
    pub async fn download_with_progress<P: AsRef<Path>>(&self, url: &str, dest: P) -> Result<()> {
        #[cfg(test)]
        if let Some(mock) = take_mock_download_with_progress() {
            return mock;
        }

        let client = self
            .client
            .as_ref()
            .expect("reqwest client available outside tests");

        info!("Downloading: {}", url);

        let response = client.get(url).send().await?;

        if !response.status().is_success() {
            return Err(crate::error::AutoInstallError::NetworkError(format!(
                "Download failed with status: {}",
                response.status()
            )));
        }

        let total_size = response.content_length().unwrap_or(0);

        let pb = ProgressBar::new(total_size);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                .unwrap()
                .progress_chars("#>-")
        );

        let mut file = File::create(&dest).await?;
        let mut stream = response.bytes_stream();
        let mut downloaded = 0u64;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            downloaded += chunk.len() as u64;
            pb.set_position(downloaded);
        }

        file.flush().await?;
        pb.finish_with_message("Download completed");

        info!("Downloaded to: {}", dest.as_ref().display());
        Ok(())
    }

    /// Download file without progress (for smaller files)
    pub async fn download<P: AsRef<Path>>(&self, url: &str, dest: P) -> Result<()> {
        #[cfg(test)]
        if let Some(mock) = take_mock_download() {
            return mock;
        }

        let client = self
            .client
            .as_ref()
            .expect("reqwest client available outside tests");

        debug!("Downloading (no progress): {}", url);

        let response = client.get(url).send().await?;

        if !response.status().is_success() {
            return Err(crate::error::AutoInstallError::NetworkError(format!(
                "Download failed with status: {}",
                response.status()
            )));
        }

        let bytes = response.bytes().await?;
        tokio::fs::write(&dest, bytes).await?;

        debug!("Downloaded to: {}", dest.as_ref().display());
        Ok(())
    }

    /// Get file size without downloading
    pub async fn get_file_size(&self, url: &str) -> Result<Option<u64>> {
        #[cfg(test)]
        if let Some(mock) = take_mock_get_file_size() {
            return mock;
        }

        let client = self
            .client
            .as_ref()
            .expect("reqwest client available outside tests");

        let response = client.head(url).send().await?;
        Ok(response.content_length())
    }

    /// Verify URL is accessible
    pub async fn verify_url(&self, url: &str) -> Result<bool> {
        #[cfg(test)]
        if let Some(mock) = take_mock_verify_url() {
            return mock;
        }

        let client = self
            .client
            .as_ref()
            .expect("reqwest client available outside tests");

        match client.head(url).send().await {
            Ok(response) => Ok(response.status().is_success()),
            Err(_) => Ok(false),
        }
    }
}

impl Default for NetworkDownloader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_verify_url() {
        super::set_mock_verify_url(Ok(true));
        let downloader = NetworkDownloader::new();
        let result = downloader.verify_url("http://unused.test").await.unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn test_get_file_size() {
        super::set_mock_get_file_size(Ok(Some(2048)));
        let downloader = NetworkDownloader::new();
        let result = downloader
            .get_file_size("http://unused.test/resource")
            .await
            .unwrap();

        assert_eq!(result, Some(2048));
    }
}
