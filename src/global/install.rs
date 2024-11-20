use super::{EnvDir, EnvironmentName, ExposedName, StateChanges};
use crate::{
    global::{
        trampoline::{Configuration, Trampoline},
        BinDir, StateChange,
    },
    prefix::Executable,
    prefix::Prefix,
};
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_utils::executable_from_path;
use rattler_conda_types::{
    MatchSpec, Matches, PackageName, ParseStrictness, Platform, RepoDataRecord,
};
use std::{collections::HashMap, path::PathBuf, str::FromStr};

use fs_err::tokio as tokio_fs;

/// Maps an entry point in the environment to a concrete `ScriptExecMapping`.
///
/// This function takes an entry point and a list of executable names and paths,
/// and returns a `ScriptExecMapping` that contains the path to the script and
/// the original executable.
/// # Returns
///
/// A `miette::Result` containing the `ScriptExecMapping` if the entry point is
/// found, or an error if it is not.
///
/// # Errors
///
/// Returns an error if the entry point is not found in the list of executable names.
pub(crate) fn script_exec_mapping<'a>(
    exposed_name: &ExposedName,
    entry_point: &str,
    mut executables: impl Iterator<Item = &'a Executable>,
    bin_dir: &BinDir,
    env_dir: &EnvDir,
) -> miette::Result<ScriptExecMapping> {
    executables
        .find(|executable| executable.name == entry_point)
        .map(|executable| ScriptExecMapping {
            global_script_path: bin_dir.executable_trampoline_path(exposed_name),
            original_executable: executable.path.clone(),
        })
        .ok_or_else(|| {
            miette::miette!(
                "Couldn't find executable {entry_point} in {}, found these executables: {:?}",
                env_dir.path().display(),
                executables.map(|exec| exec.name.clone()).collect_vec()
            )
        })
}

/// Mapping from the global script location to an executable in a package
/// environment .
#[derive(Debug)]
pub struct ScriptExecMapping {
    pub global_script_path: PathBuf,
    pub original_executable: PathBuf,
}

/// Create the executables trampolines by running the activation scripts,
/// recording this information in the trampoline metadata,
/// and saving both the trampoline and the metadata.
pub(crate) async fn create_executable_trampolines(
    mapped_executables: &[ScriptExecMapping],
    prefix: &Prefix,
    env_name: &EnvironmentName,
) -> miette::Result<StateChanges> {
    #[derive(Debug)]
    enum AddedOrChanged {
        Unchanged,
        Added,
        Changed,
        Migrated,
    }

    let mut state_changes = StateChanges::default();

    let activation_variables = prefix.run_activation().await?;

    for ScriptExecMapping {
        global_script_path,
        original_executable,
    } in mapped_executables
    {
        let exe = prefix.root().join(original_executable);
        let path = prefix
            .root()
            .join(original_executable.parent().ok_or_else(|| {
                miette::miette!(
                    "Cannot find parent directory of '{}'",
                    original_executable.display()
                )
            })?);
        let metadata = Configuration::new(exe, path, Some(activation_variables.clone()));

        let json_path = Configuration::trampoline_configuration(global_script_path);

        // Check if an old bash script is present and remove it
        let mut changed = if global_script_path.exists()
            && !Trampoline::is_trampoline(global_script_path).await?
        {
            tokio_fs::remove_file(global_script_path)
                .await
                .into_diagnostic()?;
            AddedOrChanged::Migrated
        } else if !global_script_path.exists() {
            AddedOrChanged::Added
        } else {
            AddedOrChanged::Unchanged
        };

        // Read previous metadata if it exists and update `changed` accordingly
        if matches!(changed, AddedOrChanged::Unchanged) {
            if json_path.exists() {
                let previous_manifest_data_bytes = tokio_fs::read_to_string(&json_path)
                    .await
                    .into_diagnostic()?;

                let previous_manifest_metadata: Configuration =
                    serde_json::from_str(&previous_manifest_data_bytes).into_diagnostic()?;

                changed = if previous_manifest_metadata == metadata {
                    AddedOrChanged::Unchanged
                } else {
                    AddedOrChanged::Changed
                };
            } else {
                changed = AddedOrChanged::Added;
            }
        };

        let executable_name = executable_from_path(global_script_path);
        let exposed_name = ExposedName::from_str(&executable_name)?;

        let global_script_path_parent = global_script_path.parent().ok_or_else(|| {
            miette::miette!(
                "Cannot find parent directory of '{}'",
                original_executable.display()
            )
        })?;

        let trampoline = Trampoline::new(
            exposed_name.clone(),
            global_script_path_parent.to_path_buf(),
            metadata,
        );

        match changed {
            AddedOrChanged::Unchanged => {}
            AddedOrChanged::Added => {
                trampoline.save().await?;
                state_changes.insert_change(env_name, StateChange::AddedExposed(exposed_name));
            }
            AddedOrChanged::Changed | AddedOrChanged::Migrated => {
                trampoline.save().await?;
                state_changes.insert_change(env_name, StateChange::UpdatedExposed(exposed_name));
            }
        }
    }
    Ok(state_changes)
}

/// Warn user on dangerous package installations, interactive yes no prompt
#[allow(unused)]
pub(crate) fn prompt_user_to_continue(
    packages: &IndexMap<PackageName, MatchSpec>,
) -> miette::Result<bool> {
    let dangerous_packages = HashMap::from([
        ("pixi", "Installing `pixi` globally doesn't work as expected.\nUse `pixi self-update` to update pixi and `pixi self-update --version x.y.z` for a specific version."),
        ("pip", "Installing `pip` with `pixi global` won't make pip-installed packages globally available.\nInstead, use a pixi project and add PyPI packages with `pixi add --pypi`, which is recommended. Alternatively, `pixi add pip` and use it within the project.")
    ]);

    // Check if any of the packages are dangerous, and prompt the user to ask if
    // they want to continue, including the advice.
    for (name, _spec) in packages {
        if let Some(advice) = dangerous_packages.get(&name.as_normalized()) {
            let prompt = format!(
                "{}\nDo you want to continue?",
                console::style(advice).yellow()
            );
            if !dialoguer::Confirm::new()
                .with_prompt(prompt)
                .default(false)
                .show_default(true)
                .interact()
                .into_diagnostic()?
            {
                return Ok(false);
            }
        }
    }

    Ok(true)
}

/// Checks if the local environment matches the given specifications.
///
/// This function verifies that all the given specifications are present in the
/// local environment's prefix records and that there are no extra entries in
/// the prefix records that do not match any of the specifications.
pub(crate) fn local_environment_matches_spec(
    prefix_records: Vec<RepoDataRecord>,
    specs: &IndexSet<MatchSpec>,
    platform: Option<Platform>,
) -> bool {
    // Check whether all specs in the manifest are present in the installed
    // environment
    let specs_in_manifest_are_present = specs
        .iter()
        .all(|spec| prefix_records.iter().any(|record| spec.matches(record)));

    if !specs_in_manifest_are_present {
        tracing::debug!("Not all specs in the manifest are present in the environment");
        return false;
    }

    // Check whether all packages in the installed environment have the correct
    // platform
    if let Some(platform) = platform {
        let platform_specs_match_env = prefix_records.iter().all(|record| {
            let Ok(package_platform) = Platform::from_str(&record.package_record.subdir) else {
                return true;
            };

            match package_platform {
                Platform::NoArch => true,
                p if p == platform => true,
                _ => false,
            }
        });

        if !platform_specs_match_env {
            tracing::debug!("Not all packages in the environment have the correct platform");
            return false;
        }
    }

    // Prune dependencies from the repodata that are valid for the requested specs
    fn prune_dependencies(
        mut remaining_prefix_records: Vec<RepoDataRecord>,
        matched_record: &RepoDataRecord,
    ) -> Vec<RepoDataRecord> {
        let mut work_queue = Vec::from([matched_record.as_ref().clone()]);

        while let Some(current_record) = work_queue.pop() {
            let dependencies = &current_record.depends;
            for dependency in dependencies {
                let Ok(match_spec) = MatchSpec::from_str(dependency, ParseStrictness::Lenient)
                else {
                    continue;
                };
                let Some(index) = remaining_prefix_records
                    .iter()
                    .position(|record| match_spec.matches(&record.package_record))
                else {
                    continue;
                };

                let matched_record = remaining_prefix_records.remove(index).as_ref().clone();
                work_queue.push(matched_record);
            }
        }

        remaining_prefix_records
    }

    // Process each spec and remove matched entries and their dependencies
    let remaining_prefix_records = specs.iter().fold(prefix_records, |mut acc, spec| {
        let Some(index) = acc.iter().position(|record| spec.matches(record.as_ref())) else {
            return acc;
        };
        let matched_record = acc.swap_remove(index);
        prune_dependencies(acc, &matched_record)
    });

    // If there are no remaining prefix records, then this means that
    // the environment doesn't contain records that don't match the manifest
    if !remaining_prefix_records.is_empty() {
        tracing::debug!(
            "Environment contains extra entries that don't match the manifest: {:?}",
            remaining_prefix_records
        );
        false
    } else {
        true
    }
}

/// Finds the package name in the prefix and automatically exposes it if an executable is found.
/// This is useful for packages like `ansible` and `jupyter` which don't ship executables their own executables.
/// This function will return the mapping and the package name of the package in which the binary was found.
pub async fn find_binary_by_name(
    prefix: &Prefix,
    package_name: &PackageName,
) -> miette::Result<Option<Executable>> {
    let installed_packages = prefix.find_installed_packages(None).await?;
    for package in &installed_packages {
        let executables = prefix.find_executables(&[package.clone()]);

        // Check if any of the executables match the package name
        if let Some(executable) = executables
            .iter()
            .find(|executable| executable.name.as_str() == package_name.as_normalized())
        {
            return Ok(Some(executable.clone()));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use rattler_conda_types::{MatchSpec, ParseStrictness, Platform};
    use rattler_lock::LockFile;
    use rstest::{fixture, rstest};

    use super::*;

    #[fixture]
    fn ripgrep_specs() -> IndexSet<MatchSpec> {
        IndexSet::from([MatchSpec::from_str("ripgrep=14.1.0", ParseStrictness::Strict).unwrap()])
    }

    #[fixture]
    fn ripgrep_records() -> Vec<RepoDataRecord> {
        LockFile::from_str(include_str!("./test_data/lockfiles/ripgrep.lock"))
            .unwrap()
            .default_environment()
            .unwrap()
            .conda_repodata_records(Platform::Linux64)
            .unwrap()
            .unwrap()
    }

    #[fixture]
    fn ripgrep_bat_specs() -> IndexSet<MatchSpec> {
        IndexSet::from([
            MatchSpec::from_str("ripgrep=14.1.0", ParseStrictness::Strict).unwrap(),
            MatchSpec::from_str("bat=0.24.0", ParseStrictness::Strict).unwrap(),
        ])
    }

    #[fixture]
    fn ripgrep_bat_records() -> Vec<RepoDataRecord> {
        LockFile::from_str(include_str!("./test_data/lockfiles/ripgrep_bat.lock"))
            .unwrap()
            .default_environment()
            .unwrap()
            .conda_repodata_records(Platform::Linux64)
            .unwrap()
            .unwrap()
    }

    #[rstest]
    fn test_local_environment_matches_spec(
        ripgrep_records: Vec<RepoDataRecord>,
        ripgrep_specs: IndexSet<MatchSpec>,
    ) {
        assert!(local_environment_matches_spec(
            ripgrep_records,
            &ripgrep_specs,
            None
        ));
    }

    #[rstest]
    fn test_local_environment_misses_entries_for_specs(
        mut ripgrep_records: Vec<RepoDataRecord>,
        ripgrep_specs: IndexSet<MatchSpec>,
    ) {
        // Remove last repodata record
        ripgrep_records.pop();

        assert!(!local_environment_matches_spec(
            ripgrep_records,
            &ripgrep_specs,
            None
        ));
    }

    #[rstest]
    fn test_local_environment_has_too_many_entries_to_match_spec(
        ripgrep_bat_records: Vec<RepoDataRecord>,
        ripgrep_specs: IndexSet<MatchSpec>,
        ripgrep_bat_specs: IndexSet<MatchSpec>,
    ) {
        assert!(!local_environment_matches_spec(
            ripgrep_bat_records.clone(),
            &ripgrep_specs,
            None
        ), "The function needs to detect that records coming from ripgrep and bat don't match ripgrep alone.");

        assert!(
            local_environment_matches_spec(ripgrep_bat_records, &ripgrep_bat_specs, None),
            "The records and specs match and the function should return `true`."
        );
    }

    #[rstest]
    fn test_local_environment_matches_given_platform(
        ripgrep_records: Vec<RepoDataRecord>,
        ripgrep_specs: IndexSet<MatchSpec>,
    ) {
        assert!(
            local_environment_matches_spec(
                ripgrep_records,
                &ripgrep_specs,
                Some(Platform::Linux64)
            ),
            "The records contains only linux-64 entries"
        );
    }

    #[rstest]
    fn test_local_environment_doesnt_match_given_platform(
        ripgrep_records: Vec<RepoDataRecord>,
        ripgrep_specs: IndexSet<MatchSpec>,
    ) {
        assert!(
            !local_environment_matches_spec(ripgrep_records, &ripgrep_specs, Some(Platform::Win64),),
            "The record contains linux-64 entries, so the function should always return `false`"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn test_extract_executable_from_script_windows() {
        use crate::global::trampoline::GlobalBin;
        use std::fs;
        use std::path::Path;
        let script_without_quote = r#"
@SET "PATH=C:\Users\USER\.pixi/envs\hyperfine\bin:%PATH%"
@SET "CONDA_PREFIX=C:\Users\USER\.pixi/envs\hyperfine"
@C:\Users\USER\.pixi/envs\hyperfine\bin/hyperfine.exe %*
"#;
        let script_path = Path::new("hyperfine.bat");
        let tempdir = tempfile::tempdir().unwrap();
        let script_path = tempdir.path().join(script_path);
        fs::write(&script_path, script_without_quote).unwrap();
        let script_global_bin = GlobalBin::Script(script_path);
        let executable_path = script_global_bin.executable().await.unwrap();
        assert_eq!(
            executable_path,
            Path::new("C:\\Users\\USER\\.pixi/envs\\hyperfine\\bin/hyperfine.exe")
        );

        let script_with_quote = r#"
@SET "PATH=C:\Users\USER\.pixi/envs\python\bin;%PATH%"
@SET "CONDA_PREFIX=C:\Users\USER\.pixi/envs\python"
@"C:\Users\USER\.pixi\envs\python\Scripts/pydoc.exe" %*
"#;
        let script_path = Path::new("pydoc.bat");
        let script_path = tempdir.path().join(script_path);
        fs::write(&script_path, script_with_quote).unwrap();
        let executable_path = script_global_bin.executable().await.unwrap();
        assert_eq!(
            executable_path,
            Path::new("C:\\Users\\USER\\.pixi\\envs\\hyperfine\\bin/hyperfine.exe")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_extract_executable_from_script_unix() {
        use std::{fs, path::Path};

        use crate::global::trampoline::GlobalBin;

        let script = r#"#!/bin/sh
export PATH="/home/user/.pixi/envs/nushell/bin:${PATH}"
export CONDA_PREFIX="/home/user/.pixi/envs/nushell"
"/home/user/.pixi/envs/nushell/bin/nu" "$@"
"#;
        let script_path = Path::new("nu");
        let tempdir = tempfile::tempdir().unwrap();
        let script_path = tempdir.path().join(script_path);
        fs::write(&script_path, script).unwrap();
        let script_global_bin = GlobalBin::Script(script_path);
        let executable_path = script_global_bin.executable().await.unwrap();
        assert_eq!(
            executable_path,
            Path::new("/home/user/.pixi/envs/nushell/bin/nu")
        );
    }
}
