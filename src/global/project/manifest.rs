use std::fmt;
use std::path::{Path, PathBuf};

use miette::IntoDiagnostic;
use pixi_manifest::TomlManifest;
use toml_edit::{DocumentMut, Item};

// use crate::global::document::ManifestSource;

use super::error::ManifestError;

use super::parsed_manifest::ParsedManifest;
use super::{EnvironmentName, ExposedName, MANIFEST_DEFAULT_NAME};
// use super::document::ManifestSource;

// TODO: remove
#[allow(unused)]

/// Handles the global project's manifest file.
/// This struct is responsible for reading, parsing, editing, and saving the
/// manifest. It encapsulates all logic related to the manifest's TOML format
/// and structure. The manifest data is represented as a [`ParsedManifest`]
/// struct for easy manipulation.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// The path to the manifest file
    pub path: PathBuf,

    /// Editable toml document
    pub document: TomlManifest,

    /// The parsed manifest
    pub parsed: ParsedManifest,
}

impl Manifest {
    /// Create a new manifest from a path
    pub fn from_path(path: impl AsRef<Path>) -> miette::Result<Self> {
        let manifest_path = dunce::canonicalize(path.as_ref()).into_diagnostic()?;
        let contents = std::fs::read_to_string(path.as_ref()).into_diagnostic()?;
        Self::from_str(manifest_path.as_ref(), contents)
    }

    /// Create a new manifest from a string
    pub fn from_str(manifest_path: &Path, contents: impl Into<String>) -> miette::Result<Self> {
        let contents = contents.into();
        let parsed = ParsedManifest::from_toml_str(&contents);

        let (manifest, document) = match parsed.and_then(|manifest| {
            contents
                .parse::<DocumentMut>()
                .map(|doc| (manifest, doc))
                .map_err(ManifestError::from)
        }) {
            Ok(result) => result,
            Err(e) => e.to_fancy(MANIFEST_DEFAULT_NAME, &contents)?,
        };

        let manifest = Self {
            path: manifest_path.to_path_buf(),
            document: TomlManifest::new(document),
            parsed: manifest,
        };

        Ok(manifest)
    }

    pub fn add_exposed_mapping(
        &mut self,
        env_name: &EnvironmentName,
        mapping: &Mapping,
    ) -> miette::Result<()> {
        self.document
            .get_or_insert_nested_table(&format!("envs.{env_name}.exposed"))?
            .insert(
                &mapping.exposed_name.to_string(),
                Item::Value(toml_edit::Value::from(mapping.executable_name.clone())),
            );

        tracing::debug!("Added exposed mapping {mapping} to toml document");
        Ok(())
    }

    pub fn remove_exposed_name(
        &mut self,
        env_name: &EnvironmentName,
        exposed_name: &ExposedName,
    ) -> miette::Result<()> {
        self.document
            .get_or_insert_nested_table(&format!("envs.{env_name}.exposed"))?
            .remove(&exposed_name.to_string())
            .ok_or_else(|| miette::miette!("The exposed name {exposed_name} doesn't exist"))?;

        tracing::debug!("Removed exposed mapping {exposed_name} from toml document");
        Ok(())
    }

    /// Save the manifest to the file and update the parsed_manifest
    pub async fn save(&mut self) -> miette::Result<()> {
        let contents = self.document.to_string();
        self.parsed = ParsedManifest::from_toml_str(&contents)?;
        tokio::fs::write(&self.path, contents)
            .await
            .into_diagnostic()?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Mapping {
    exposed_name: ExposedName,
    executable_name: String,
}

impl Mapping {
    pub fn new(exposed_name: ExposedName, executable_name: String) -> Self {
        Self {
            exposed_name,
            executable_name,
        }
    }
}

impl fmt::Display for Mapping {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}={}", self.exposed_name, self.executable_name)
    }
}
