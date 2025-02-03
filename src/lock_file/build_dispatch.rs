//! This module contains an implementation of the `BuildContext` trait for the `LazyBuildDispatch` trait.
//! This is mainly to be able to initialize the conda prefix for PyPI resolving on demand.
//! This is needed because the conda prefix is a heavy operation and we want to avoid initializing it.
//! And we do not need to initialize it if we are not resolving PyPI source dependencies.
//! With this implementation we only initialize a prefix once uv requests some operation that actually needs this prefix.
//!
//! This is especially prudent to do when we have multiple environments, which translates into multiple prefixes, that all need to be initialized.
//! Previously we would initialize all prefixes upfront, but this is not needed and can also sometimes not be done for each platform.
//! Using this implementation we can solve for a lot more platforms than we could before.
//!
//! The main struct of interest is the [`LazyBuildDispatch`] struct which holds the parameters needed to create a `BuildContext` uv implementation.
//! and holds struct that is used to instantiate the conda prefix when its needed.
use std::{path::Path, sync::Arc};

use async_once_cell::OnceCell as AsyncCell;

use once_cell::sync::OnceCell;

use anyhow::Result;
use pixi_manifest::EnvironmentName;
use pixi_uv_conversions::{isolated_names_to_packages, names_to_build_isolation};
use std::collections::HashMap;
use tokio::runtime::Handle;
use uv_build_frontend::SourceBuild;
use uv_cache::Cache;
use uv_client::RegistryClient;
use uv_configuration::{
    BuildKind, BuildOptions, BuildOutput, Concurrency, ConfigSettings, Constraints, IndexStrategy,
    LowerBound, SourceStrategy,
};
use uv_dispatch::{BuildDispatch, SharedState};
use uv_distribution_types::{
    CachedDist, DependencyMetadata, IndexLocations, Resolution, SourceDist,
};
use uv_install_wheel::linker::LinkMode;
use uv_pep508::PackageName;
use uv_pypi_types::Requirement;
use uv_python::{Interpreter, PythonEnvironment};
use uv_resolver::{ExcludeNewer, FlatIndex};
use uv_types::{BuildContext, HashStrategy};

use crate::{
    activation::CurrentEnvVarBehavior,
    project::{get_activated_environment_variables, Environment, EnvironmentVars},
};

use super::{conda_prefix_updater::CondaPrefixUpdated, CondaPrefixUpdater, PixiRecordsByName};

/// This structure holds all the parameters needed to create a `BuildContext` uv implementation.
pub struct UvBuildDispatchParams<'a> {
    client: &'a RegistryClient,
    cache: &'a Cache,
    index_locations: &'a IndexLocations,
    flat_index: &'a FlatIndex,
    dependency_metadata: &'a DependencyMetadata,
    config_settings: &'a ConfigSettings,
    build_options: &'a BuildOptions,
    hasher: &'a HashStrategy,
    index_strategy: IndexStrategy,
    constraints: Constraints,
    shared_state: SharedState,
    link_mode: uv_install_wheel::linker::LinkMode,
    exclude_newer: Option<ExcludeNewer>,
    bounds: LowerBound,
    sources: SourceStrategy,
    concurrency: Concurrency,
}

impl<'a> UvBuildDispatchParams<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: &'a RegistryClient,
        cache: &'a Cache,
        index_locations: &'a IndexLocations,
        flat_index: &'a FlatIndex,
        dependency_metadata: &'a DependencyMetadata,
        config_settings: &'a ConfigSettings,
        build_options: &'a BuildOptions,
        hasher: &'a HashStrategy,
    ) -> Self {
        Self {
            client,
            cache,
            index_locations,
            flat_index,
            dependency_metadata,
            config_settings,
            build_options,
            hasher,
            index_strategy: IndexStrategy::default(),
            shared_state: SharedState::default(),
            link_mode: LinkMode::default(),
            constraints: Constraints::default(),
            exclude_newer: None,
            bounds: LowerBound::default(),
            sources: SourceStrategy::default(),
            concurrency: Concurrency::default(),
        }
    }

    pub fn with_index_strategy(mut self, index_strategy: IndexStrategy) -> Self {
        self.index_strategy = index_strategy;
        self
    }

    pub fn with_shared_state(mut self, shared_state: SharedState) -> Self {
        self.shared_state = shared_state;
        self
    }

    pub fn with_source_strategy(mut self, sources: SourceStrategy) -> Self {
        self.sources = sources;
        self
    }

    pub fn with_concurrency(mut self, concurrency: Concurrency) -> Self {
        self.concurrency = concurrency;
        self
    }

    #[allow(dead_code)]
    pub fn with_link_mode(mut self, link_mode: LinkMode) -> Self {
        self.link_mode = link_mode;
        self
    }

    #[allow(dead_code)]
    pub fn with_constraints(mut self, constraints: Constraints) -> Self {
        self.constraints = constraints;
        self
    }

    #[allow(dead_code)]
    pub fn with_exclude_newer(mut self, exclude_newer: Option<ExcludeNewer>) -> Self {
        self.exclude_newer = exclude_newer;
        self
    }

    #[allow(dead_code)]
    pub fn with_lower_bounds(mut self, lower_bounds: LowerBound) -> Self {
        self.bounds = lower_bounds;
        self
    }
}

/// Handles the lazy initialization of a build dispatch.
///
/// A build dispatch is used to manage building Python packages from source,
/// including setting up build environments, dependencies, and executing builds.
///
/// This struct helps manage resources needed for build dispatch that may need
/// to be initialized on-demand rather than upfront.
///
/// Both the [`BuildDispatch`] and the conda prefix are instantiated on demand.
pub struct LazyBuildDispatch<'a> {
    pub params: UvBuildDispatchParams<'a>,
    pub prefix_updater: CondaPrefixUpdater<'a>,
    pub repodata_records: Arc<PixiRecordsByName>,

    pub build_dispatch: AsyncCell<BuildDispatch<'a>>,

    // if we create a new conda prefix, we need to store the task result
    // so we could reuse it later
    pub conda_task: Option<CondaPrefixUpdated>,

    // project environment variables
    // this is used to get the activated environment variables
    pub project_env_vars: HashMap<EnvironmentName, EnvironmentVars>,
    pub environment: Environment<'a>,

    // what pkgs we dont need to activate
    pub no_build_isolation: Option<Vec<String>>,

    // we need to tie the interpreter to the build dispatch
    pub lazy_deps: &'a LazyBuildDispatchDependencies,
}

/// These are resources for the [`BuildDispatch`] that need to be lazily initialized.
/// along with the build dispatch.
///
/// This needs to be passed in externally or there will be problems with the borrows being shorter
/// than the lifetime of the `BuildDispatch`, and we are returning the references.
#[derive(Default)]
pub struct LazyBuildDispatchDependencies {
    /// The initialized python interpreter
    interpreter: OnceCell<Interpreter>,
    /// The non isolated packages
    non_isolated_packages: OnceCell<Option<Vec<PackageName>>>,
    /// The python environment
    python_env: OnceCell<PythonEnvironment>,
}

impl<'a> LazyBuildDispatch<'a> {
    /// Create a new `PixiBuildDispatch` instance.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        params: UvBuildDispatchParams<'a>,
        prefix_updater: CondaPrefixUpdater<'a>,
        project_env_vars: HashMap<EnvironmentName, EnvironmentVars>,
        environment: Environment<'a>,
        repodata_records: Arc<PixiRecordsByName>,
        no_build_isolation: Option<Vec<String>>,
        lazy_deps: &'a LazyBuildDispatchDependencies,
    ) -> Self {
        Self {
            params,
            prefix_updater,
            conda_task: None,
            project_env_vars,
            environment,
            repodata_records,
            no_build_isolation,
            build_dispatch: AsyncCell::new(),
            lazy_deps,
        }
    }

    /// Lazy initialization of the `BuildDispatch`. This also implies initializing the conda prefix.
    async fn get_or_try_init(&self) -> anyhow::Result<&BuildDispatch> {
        self.build_dispatch
            .get_or_try_init(async {
                tracing::debug!(
                    "installing conda prefix {} for solving the pypi sdist requirements",
                    self.prefix_updater.group.name().as_str()
                );
                let prefix = self
                    .prefix_updater
                    .update(self.repodata_records.clone())
                    .await
                    .map_err(|err| {
                        anyhow::anyhow!(err).context("failed to install conda prefix")
                    })?;

                // get the activation vars
                let env_vars = get_activated_environment_variables(
                    &self.project_env_vars,
                    &self.environment,
                    CurrentEnvVarBehavior::Exclude,
                    None,
                    false,
                    false,
                )
                .await
                .map_err(|err| {
                    anyhow::anyhow!(err).context("failed to get activated environment variables")
                })?;

                let python_path = prefix
                    .python_status
                    .location()
                    .map(|path| prefix.prefix.root().join(path))
                    .ok_or_else(|| {
                        anyhow::anyhow!(format!(
                            "missing python interpreter from conda prefix {}. \n {}",
                            prefix.prefix.root().display(),
                            "Use `pixi add python` to install the latest python interpreter.",
                        ))
                    })?;

                let interpreter = self
                    .lazy_deps
                    .interpreter
                    .get_or_try_init(|| Interpreter::query(python_path, self.cache()))?;

                let env = self
                    .lazy_deps
                    .python_env
                    .get_or_init(|| PythonEnvironment::from_interpreter(interpreter.clone()));

                let non_isolated_packages =
                    self.lazy_deps.non_isolated_packages.get_or_try_init(|| {
                        isolated_names_to_packages(self.no_build_isolation.as_deref()).map_err(
                            |err| {
                                anyhow::anyhow!(err).context("failed to get non isolated packages")
                            },
                        )
                    })?;

                let build_isolation =
                    names_to_build_isolation(non_isolated_packages.as_deref(), env);

                let build_dispatch = BuildDispatch::new(
                    self.params.client,
                    self.params.cache,
                    self.params.constraints.clone(),
                    interpreter,
                    self.params.index_locations,
                    self.params.flat_index,
                    self.params.dependency_metadata,
                    self.params.shared_state.clone(),
                    self.params.index_strategy,
                    self.params.config_settings,
                    build_isolation,
                    self.params.link_mode,
                    self.params.build_options,
                    self.params.hasher,
                    self.params.exclude_newer,
                    self.params.bounds,
                    self.params.sources,
                    self.params.concurrency,
                )
                .with_build_extra_env_vars(env_vars);

                Ok(build_dispatch)
            })
            .await
    }
}

impl BuildContext for LazyBuildDispatch<'_> {
    type SourceDistBuilder = SourceBuild;

    fn interpreter(&self) -> &uv_python::Interpreter {
        // In most cases the interpreter should be initialized, because one of the other trait
        // methods will have been called
        // But in case it is not, we will initialize it here
        //
        // Even though intitalize does not initialize twice, we skip the codepath because the initialization takes time
        if self.lazy_deps.interpreter.get().is_none() {
            // This will usually be called from the multi-threaded runtime, but there might be tests
            // that calls this in the current thread runtime.
            // In the current thread runtime we cannot use `block_in_place` as it will pani
            let handle = Handle::current();
            match handle.runtime_flavor() {
                tokio::runtime::RuntimeFlavor::CurrentThread => {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("failed to initialize the runtime ");
                    runtime
                        .block_on(self.get_or_try_init())
                        .expect("failed to initialize the build dispatch");
                }
                // Others are multi-threaded runtimes
                _ => {
                    tokio::task::block_in_place(move || {
                        handle
                            .block_on(self.get_or_try_init())
                            .expect("failed to initialize build dispatch");
                    });
                }
            }
        }
        self.lazy_deps
            .interpreter
            .get()
            .expect("python interpreter not initialized, this is a programming error")
    }

    fn cache(&self) -> &uv_cache::Cache {
        self.params.cache
    }

    fn git(&self) -> &uv_git::GitResolver {
        self.params.shared_state.git()
    }

    fn capabilities(&self) -> &uv_distribution_types::IndexCapabilities {
        self.params.shared_state.capabilities()
    }

    fn dependency_metadata(&self) -> &uv_distribution_types::DependencyMetadata {
        self.params.dependency_metadata
    }

    fn build_options(&self) -> &uv_configuration::BuildOptions {
        self.params.build_options
    }

    fn config_settings(&self) -> &uv_configuration::ConfigSettings {
        self.params.config_settings
    }

    fn bounds(&self) -> uv_configuration::LowerBound {
        self.params.bounds
    }

    fn sources(&self) -> uv_configuration::SourceStrategy {
        self.params.sources
    }

    fn locations(&self) -> &uv_distribution_types::IndexLocations {
        self.params.index_locations
    }

    async fn resolve<'data>(&'data self, requirements: &'data [Requirement]) -> Result<Resolution> {
        self.get_or_try_init().await?.resolve(requirements).await
    }

    async fn install<'data>(
        &'data self,
        resolution: &'data Resolution,
        venv: &'data PythonEnvironment,
    ) -> Result<Vec<CachedDist>> {
        self.get_or_try_init()
            .await?
            .install(resolution, venv)
            .await
    }

    async fn setup_build<'data>(
        &'data self,
        source: &'data Path,
        subdirectory: Option<&'data Path>,
        install_path: &'data Path,
        version_id: Option<String>,
        dist: Option<&'data SourceDist>,
        sources: SourceStrategy,
        build_kind: BuildKind,
        build_output: BuildOutput,
    ) -> Result<SourceBuild> {
        self.get_or_try_init()
            .await?
            .setup_build(
                source,
                subdirectory,
                install_path,
                version_id,
                dist,
                sources,
                build_kind,
                build_output,
            )
            .await
    }
}
