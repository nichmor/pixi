use std::{collections::HashMap, sync::Arc, time::Duration};

use indicatif::{MultiProgress, ProgressBar};
use parking_lot::Mutex;
use pixi_build_frontend::{CondaBuildReporter, CondaMetadataReporter};

pub trait BuildMetadataReporter: CondaMetadataReporter {
    /// Reporters that the metadata has been cached.
    fn on_metadata_cached(&self, build_id: usize);

    /// Cast upwards
    fn as_conda_metadata_reporter(self: Arc<Self>) -> Arc<dyn CondaMetadataReporter>;
}

/// Noop implementation of the BuildMetadataReporter trait.
struct NoopBuildMetadataReporter;
impl CondaMetadataReporter for NoopBuildMetadataReporter {
    fn on_metadata_start(&self, _build_id: usize) -> usize {
        0
    }

    fn on_metadata_end(&self, _operation: usize) {}
}
impl BuildMetadataReporter for NoopBuildMetadataReporter {
    fn on_metadata_cached(&self, _build_id: usize) {}

    fn as_conda_metadata_reporter(self: Arc<Self>) -> Arc<dyn CondaMetadataReporter> {
        self
    }
}

pub trait BuildReporter: CondaBuildReporter {
    /// Reports that the build has been cached.
    fn on_build_cached(&self, build_id: usize);

    /// Cast upwards
    fn as_conda_build_reporter(self: Arc<Self>) -> Arc<dyn CondaBuildReporter>;
}

/// Noop implementation of the BuildReporter trait.
struct NoopBuildReporter;
impl CondaBuildReporter for NoopBuildReporter {
    fn on_build_start(&self, _build_id: usize) -> usize {
        0
    }

    fn on_build_end(&self, _operation: usize) {}

    fn on_build_output(&self, _operation: usize, _line: String) {}
}
impl BuildReporter for NoopBuildReporter {
    fn on_build_cached(&self, _build_id: usize) {}

    fn as_conda_build_reporter(self: Arc<Self>) -> Arc<dyn CondaBuildReporter> {
        self
    }
}

/// A reporter trait that it is responsible for reporting the progress of some source checkout.
pub trait SourceReporter: pixi_git::Reporter {
    /// Cast upwards
    fn as_git_reporter(self: Arc<Self>) -> Arc<dyn pixi_git::Reporter>;
}

#[derive(Default, Debug)]
struct ProgressState {
    /// A map of progress bars, by ID.
    bars: HashMap<usize, ProgressBar>,
    /// A monotonic counter for bar IDs.
    id: usize,
}

impl ProgressState {
    /// Returns a unique ID for a new progress bar.
    fn id(&mut self) -> usize {
        self.id += 1;
        self.id
    }
}

pub struct SourceCheckoutReporter {
    multi_progress: MultiProgress,
    progress_state: Arc<Mutex<ProgressState>>,
}

impl SourceCheckoutReporter {
    pub fn new(multi_progress: MultiProgress) -> Self {
        Self {
            multi_progress,
            progress_state: Default::default(),
        }
    }

    /// Similar to the default pixi_progress::default_progress_style, but with a spinner in front.
    pub fn spinner_style() -> indicatif::ProgressStyle {
        indicatif::ProgressStyle::with_template("{spinner:.green} {prefix} {wide_msg:.dim}")
            .unwrap()
    }
}

impl pixi_git::Reporter for SourceCheckoutReporter {
    fn on_checkout_start(&self, url: &url::Url, rev: &str) -> usize {
        let mut state = self.progress_state.lock();
        let id = state.id();

        let pb = self.multi_progress.add(ProgressBar::hidden());

        pb.set_style(SourceCheckoutReporter::spinner_style());

        pb.set_prefix("fetching git dependencies");

        pb.set_message(format!("checking out {}@{}", url, rev));
        pb.enable_steady_tick(Duration::from_millis(100));

        state.bars.insert(id, pb);

        id
    }

    fn on_checkout_complete(&self, url: &url::Url, rev: &str, index: usize) {
        let mut state = self.progress_state.lock();
        let removed_pb = state.bars.remove(&index).unwrap();

        removed_pb.finish_with_message(format!("checkout complete {}@{}", url, rev));
        removed_pb.finish_and_clear();
    }
}

impl SourceReporter for SourceCheckoutReporter {
    fn as_git_reporter(self: Arc<Self>) -> Arc<dyn pixi_git::Reporter> {
        self
    }
}
