use std::path::PathBuf;

use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_conda_types::{Channel, ChannelConfig, PackageName, PrefixRecord};

use crate::{prefix::Prefix, repodata};
use pixi_config::home_path;

/// Global binaries directory, default to `$HOME/.pixi/bin`
pub struct BinDir(pub PathBuf);

impl BinDir {
    /// Create the Binary Executable directory
    pub async fn create() -> miette::Result<Self> {
        let bin_dir = bin_dir().ok_or(miette::miette!(
            "could not determine global binary executable directory"
        ))?;
        tokio::fs::create_dir_all(&bin_dir)
            .await
            .into_diagnostic()?;
        Ok(Self(bin_dir))
    }

    /// Get the Binary Executable directory, erroring if it doesn't already
    /// exist.
    pub async fn from_existing() -> miette::Result<Self> {
        let bin_dir = bin_dir().ok_or(miette::miette!(
            "could not find global binary executable directory"
        ))?;
        if tokio::fs::try_exists(&bin_dir).await.into_diagnostic()? {
            Ok(Self(bin_dir))
        } else {
            Err(miette::miette!(
                "binary executable directory does not exist"
            ))
        }
    }
}

/// Global binary environments directory, default to `$HOME/.pixi/envs`
pub struct BinEnvDir(pub PathBuf);

impl BinEnvDir {
    /// Construct the path to the env directory for the binary package
    /// `package_name`.
    fn package_bin_env_dir(package_name: &PackageName) -> miette::Result<PathBuf> {
        Ok(bin_env_dir()
            .ok_or(miette::miette!(
                "could not find global binary environment directory"
            ))?
            .join(package_name.as_normalized()))
    }

    /// Get the Binary Environment directory, erroring if it doesn't already
    /// exist.
    pub async fn from_existing(package_name: &PackageName) -> miette::Result<Self> {
        let bin_env_dir = Self::package_bin_env_dir(package_name)?;
        if tokio::fs::try_exists(&bin_env_dir)
            .await
            .into_diagnostic()?
        {
            Ok(Self(bin_env_dir))
        } else {
            Err(miette::miette!(
                "could not find environment for package {}",
                package_name.as_source()
            ))
        }
    }

    /// Create the Binary Environment directory
    pub async fn create(package_name: &PackageName) -> miette::Result<Self> {
        let bin_env_dir = Self::package_bin_env_dir(package_name)?;
        tokio::fs::create_dir_all(&bin_env_dir)
            .await
            .into_diagnostic()?;
        Ok(Self(bin_env_dir))
    }
}

/// Global binaries directory, default to `$HOME/.pixi/bin`
///
/// # Returns
///
/// The global binaries directory
pub(crate) fn bin_dir() -> Option<PathBuf> {
    home_path().map(|path| path.join("bin"))
}

/// Global binary environments directory, default to `$HOME/.pixi/envs`
///
/// # Returns
///
/// The global binary environments directory
pub(crate) fn bin_env_dir() -> Option<PathBuf> {
    home_path().map(|path| path.join("envs"))
}

/// Get the friendly channel name of a [`PrefixRecord`]
///
/// # Returns
///
/// The friendly channel name of the given prefix record
pub(crate) fn channel_name_from_prefix(
    prefix_package: &PrefixRecord,
    channel_config: &ChannelConfig,
) -> String {
    Channel::from_str(&prefix_package.repodata_record.channel, channel_config)
        .map(|ch| repodata::friendly_channel_name(&ch))
        .unwrap_or_else(|_| prefix_package.repodata_record.channel.clone())
}

/// Find the globally installed package with the given [`PackageName`]
///
/// # Returns
///
/// The PrefixRecord of the installed package
pub(crate) async fn find_installed_package(
    package_name: &PackageName,
) -> miette::Result<PrefixRecord> {
    let BinEnvDir(bin_prefix) = BinEnvDir::from_existing(package_name).await.or_else(|_| {
        miette::bail!(
            "Package {} is not globally installed",
            package_name.as_source()
        )
    })?;
    let prefix = Prefix::new(bin_prefix);
    find_designated_package(&prefix, package_name).await
}

/// Find the designated package in the given [`Prefix`]
///
/// # Returns
///
/// The PrefixRecord of the designated package
pub(crate) async fn find_designated_package(
    prefix: &Prefix,
    package_name: &PackageName,
) -> miette::Result<PrefixRecord> {
    let prefix_records = prefix.find_installed_packages(None).await?;
    prefix_records
        .into_iter()
        .find(|r| r.repodata_record.package_record.name == *package_name)
        .ok_or_else(|| miette::miette!("could not find {} in prefix", package_name.as_source()))
}

pub(crate) async fn print_executables_available(executables: Vec<PathBuf>) -> miette::Result<()> {
    let BinDir(bin_dir) = BinDir::from_existing().await?;
    let whitespace = console::Emoji("  ", "").to_string();
    let executable = executables
        .into_iter()
        .map(|path| {
            path.strip_prefix(&bin_dir)
                .expect("script paths were constructed by joining onto BinDir")
                .to_string_lossy()
                .to_string()
        })
        .join(&format!("\n{whitespace} -  "));

    if is_bin_folder_on_path().await {
        eprintln!(
            "{whitespace}These executables are now globally available:\n{whitespace} -  {executable}",
        )
    } else {
        eprintln!("{whitespace}These executables have been added to {}\n{whitespace} -  {executable}\n\n{} To use them, make sure to add {} to your PATH",
                  console::style(&bin_dir.display()).bold(),
                  console::style("!").yellow().bold(),
                  console::style(&bin_dir.display()).bold()
        )
    }

    Ok(())
}

/// Returns true if the bin folder is available on the PATH.
async fn is_bin_folder_on_path() -> bool {
    let bin_path = match BinDir::from_existing().await.ok() {
        Some(BinDir(bin_dir)) => bin_dir,
        None => return false,
    };

    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect_vec())
        .unwrap_or_default()
        .into_iter()
        .contains(&bin_path)
}
