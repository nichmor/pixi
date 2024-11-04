// use uv_normalize::PackageName;
// use pep508_rs::PackageName;

use once_cell::sync::Lazy;
use std::error::Error;
use std::{
    fmt::{Debug, Display},
    str::FromStr,
    sync::Arc,
};
use thiserror::Error;
use uv_distribution_types::Name;
use uv_pypi_types::VerbatimParsedUrl;

#[derive(Debug)]
pub enum NameError {
    PepNameError(pep508_rs::InvalidNameError),
    PepExtraNameError(pep508_rs::InvalidNameError),
    UvNameError(uv_normalize::InvalidNameError),
    UvExtraNameError(uv_normalize::InvalidNameError),
}

impl Display for NameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NameError::PepNameError(e) => write!(f, "Failed to convert to pep name {}", e),
            NameError::UvNameError(e) => write!(f, "Failed to convert to uv name  {}", e),
            NameError::PepExtraNameError(e) => {
                write!(f, "Failed to convert to uv extra name  {}", e)
            }
            NameError::UvExtraNameError(e) => {
                write!(f, "Failed to convert to uv extra name  {}", e)
            }
        }
    }
}

impl Error for NameError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            NameError::PepNameError(e) => Some(e),
            NameError::UvNameError(e) => Some(e),
            NameError::PepExtraNameError(e) => Some(e),
            NameError::UvExtraNameError(e) => Some(e),
        }
    }
}

#[derive(Debug)]
pub enum VersionSpecifiersError {
    PepVersionError(pep440_rs::VersionSpecifiersParseError),
    UvVersionError(uv_pep440::VersionSpecifiersParseError),
}

impl Display for VersionSpecifiersError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VersionSpecifiersError::PepVersionError(e) => {
                write!(f, "Failed to convert to pep version {}", e)
            }
            VersionSpecifiersError::UvVersionError(e) => {
                write!(f, "Failed to convert to uv version  {}", e)
            }
        }
    }
}

impl Error for VersionSpecifiersError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            VersionSpecifiersError::PepVersionError(e) => Some(e),
            VersionSpecifiersError::UvVersionError(e) => Some(e),
        }
    }
}

#[derive(Debug)]
pub enum Pep508Error {
    Pep508Error(pep508_rs::Pep508Error),
    UvPep508(uv_pep508::Pep508Error),
}

impl Display for Pep508Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Pep508Error::Pep508Error(e) => write!(f, "Failed to convert {}", e),
            Pep508Error::UvPep508(e) => write!(f, "Failed to convert to convert {}", e),
        }
    }
}

impl Error for Pep508Error {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Pep508Error::Pep508Error(e) => Some(e),
            Pep508Error::UvPep508(e) => Some(e),
        }
    }
}

// Define a custom error type for all potential conversion errors
#[derive(Error, Debug)]
pub enum ConversionError {
    #[error(transparent)]
    InvalidPackageName(#[from] NameError),

    #[error(transparent)]
    Pep508Error(#[from] Pep508Error),

    #[error("Invalid marker environment serialization")]
    MarkerEnvironmentSerialization(#[from] serde_json::Error),

    #[error(transparent)]
    InvalidVersionSpecifier(#[from] VersionSpecifiersError),

    #[error(transparent)]
    InvalidVersion(#[from] uv_pep440::VersionParseError),
}

pub fn to_requirements<'req>(
    requirements: impl Iterator<Item = &'req uv_pypi_types::Requirement>,
) -> Result<Vec<pep508_rs::Requirement>, ConversionError> {
    let requirements: Result<Vec<pep508_rs::Requirement>, _> = requirements
        .map(|requirement| {
            let requirement: uv_pep508::Requirement<VerbatimParsedUrl> =
                uv_pep508::Requirement::from(requirement.clone());
            pep508_rs::Requirement::from_str(&requirement.to_string())
                .map_err(|err| Pep508Error::Pep508Error(err))
        })
        .collect();

    Ok(requirements?)
}

/// Convert back to PEP508 without the VerbatimParsedUrl
/// We need this function because we need to convert to the introduced
/// `VerbatimParsedUrl` back to crates.io `VerbatimUrl`, for the locking
pub fn convert_uv_requirements_to_pep508<'req>(
    requires_dist: impl Iterator<Item = &'req uv_pep508::Requirement<VerbatimParsedUrl>>,
) -> Result<Vec<pep508_rs::Requirement>, ConversionError> {
    // Convert back top PEP508 Requirement<VerbatimUrl>
    let requirements: Result<Vec<pep508_rs::Requirement>, _> = requires_dist
        .map(|r| {
            let requirement = r.to_string();
            pep508_rs::Requirement::from_str(&requirement)
                .map_err(|err| Pep508Error::Pep508Error(err))
        })
        .collect();

    Ok(requirements?)
}

/// Converts `uv_normalize::PackageName` to `pep508_rs::PackageName`
pub fn to_normalize(
    normalise: &uv_normalize::PackageName,
) -> Result<pep508_rs::PackageName, ConversionError> {
    Ok(pep508_rs::PackageName::from_str(normalise.as_str())
        .map_err(|err| NameError::PepNameError(err))?)
}

/// Converts `pe508::PackageName` to  `uv_normalize::PackageName`
pub fn to_uv_normalize(
    normalise: &pep508_rs::PackageName,
) -> Result<uv_normalize::PackageName, ConversionError> {
    Ok(
        uv_normalize::PackageName::from_str(normalise.to_string().as_str())
            .map_err(|err| NameError::UvNameError(err))?,
    )
}

/// Converts `pep508_rs::ExtraName` to `uv_normalize::ExtraName`
pub fn to_uv_extra_name(
    extra_name: &pep508_rs::ExtraName,
) -> Result<uv_normalize::ExtraName, ConversionError> {
    Ok(
        uv_normalize::ExtraName::from_str(extra_name.to_string().as_str())
            .map_err(|err| NameError::UvExtraNameError(err))?,
    )
}

/// Converts `uv_normalize::ExtraName` to `pep508_rs::ExtraName`
pub fn to_extra_name(
    extra_name: &uv_normalize::ExtraName,
) -> Result<pep508_rs::ExtraName, ConversionError> {
    Ok(
        pep508_rs::ExtraName::from_str(extra_name.to_string().as_str())
            .map_err(|err| NameError::PepExtraNameError(err))?,
    )
}

/// Converts `uv_normalize::ExtraName` to `pep508_rs::ExtraName`
pub fn to_uv_version(version: &pep440_rs::Version) -> Result<uv_pep440::Version, ConversionError> {
    Ok(uv_pep440::Version::from_str(version.to_string().as_str())
        .map_err(|err| ConversionError::InvalidVersion(err))?)
}

/// Converts `pep508_rs::MarkerTree` to `uv_pep508::MarkerTree`
pub fn to_uv_marker_tree(
    marker_tree: &pep508_rs::MarkerTree,
) -> Result<uv_pep508::MarkerTree, ConversionError> {
    let serialized = marker_tree.try_to_string();
    if let Some(serialized) = serialized {
        Ok(uv_pep508::MarkerTree::from_str(serialized.as_str())
            .map_err(|err| Pep508Error::UvPep508(err))?)
    } else {
        Ok(uv_pep508::MarkerTree::default())
    }
}

/// Converts `pep508_rs::MarkerTree` to `uv_pep508::MarkerTree`
pub fn to_marker_environment(
    marker_env: &uv_pep508::MarkerEnvironment,
) -> Result<pep508_rs::MarkerEnvironment, ConversionError> {
    let serde_str = serde_json::to_string(marker_env).expect("its valid");
    Ok(serde_json::from_str(&serde_str)
        .map_err(|err| ConversionError::MarkerEnvironmentSerialization(err))?)
}

pub fn to_uv_version_specifiers(
    version_specifier: &pep440_rs::VersionSpecifiers,
) -> Result<uv_pep440::VersionSpecifiers, ConversionError> {
    Ok(
        uv_pep440::VersionSpecifiers::from_str(&version_specifier.to_string())
            .map_err(|err| VersionSpecifiersError::UvVersionError(err))?,
    )
}

pub fn to_version_specifiers(
    version_specifier: &uv_pep440::VersionSpecifiers,
) -> Result<pep440_rs::VersionSpecifiers, ConversionError> {
    Ok(
        pep440_rs::VersionSpecifiers::from_str(&version_specifier.to_string())
            .map_err(|err| VersionSpecifiersError::PepVersionError(err))?,
    )
}
