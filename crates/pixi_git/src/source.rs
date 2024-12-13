use std::{
    borrow::Cow,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
};

use reqwest_middleware::ClientWithMiddleware;
use tracing::{debug, instrument};

use crate::{
    credentials::GIT_STORE,
    git::GitRemote,
    sha::{GitOid, GitSha},
    url::RepositoryUrl,
    GitUrl,
};

/// A remote Git source that can be checked out locally.
pub struct GitSource {
    /// The Git reference from the manifest file.
    git: GitUrl,
    /// The HTTP client to use for fetching.
    client: ClientWithMiddleware,
    /// The path to the Git source database.
    cache: PathBuf,
}

impl GitSource {
    /// Initialize a new Git source.
    pub fn new(
        git: GitUrl,
        client: impl Into<ClientWithMiddleware>,
        cache: impl Into<PathBuf>,
    ) -> Self {
        Self {
            git,
            client: client.into(),
            cache: cache.into(),
        }
    }

    /// Fetch the underlying Git repository at the given revision.
    #[instrument(skip(self), fields(repository = %self.git.repository, rev = ?self.git.precise))]
    pub fn fetch(self) -> miette::Result<Fetch> {
        // Compute the canonical URL for the repository.
        let canonical = RepositoryUrl::new(&self.git.repository);

        // The path to the repo, within the Git database.
        let ident = cache_digest(&canonical);
        let db_path = self.cache.join("db").join(&ident);

        // Authenticate the URL, if necessary.
        let remote = if let Some(credentials) = GIT_STORE.get(&canonical) {
            Cow::Owned(credentials.apply(self.git.repository.clone()))
        } else {
            Cow::Borrowed(&self.git.repository)
        };

        let remote = GitRemote::new(&remote);
        let (db, actual_rev) = match (self.git.precise, remote.db_at(&db_path).ok()) {
            // If we have a locked revision, and we have a preexisting database
            // which has that revision, then no update needs to happen.
            (Some(rev), Some(db)) if db.contains(rev.into()) => {
                debug!("Using existing Git source `{}`", self.git.repository);
                (db, rev)
            }

            // ... otherwise we use this state to update the git database. Note
            // that we still check for being offline here, for example in the
            // situation that we have a locked revision but the database
            // doesn't have it.
            (locked_rev, db) => {
                debug!("Updating Git source `{}`", self.git.repository);

                let (db, actual_rev) = remote.checkout(
                    &db_path,
                    db,
                    &self.git.reference,
                    locked_rev.map(GitOid::from),
                    &self.client,
                )?;

                (db, GitSha::from(actual_rev))
            }
        };

        // Don’t use the full hash, in order to contribute less to reaching the
        // path length limit on Windows.
        let short_id = db.to_short_id(actual_rev.into())?;

        // Check out `actual_rev` from the database to a scoped location on the
        // filesystem. This will use hard links and such to ideally make the
        // checkout operation here pretty fast.
        let checkout_path = self
            .cache
            .join("checkouts")
            .join(&ident)
            .join(short_id.as_str());

        debug!(" I will copy from {:?} to {:?}", actual_rev, checkout_path);
        db.copy_to(actual_rev.into(), &checkout_path)?;

        Ok(Fetch {
            git: self.git.with_precise(actual_rev),
            path: checkout_path,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Fetch {
    /// The [`GitUrl`] reference that was fetched.
    git: GitUrl,
    /// The path to the checked out repository.
    path: PathBuf,
}

impl Fetch {
    pub fn git(&self) -> &GitUrl {
        &self.git
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn into_git(self) -> GitUrl {
        self.git
    }

    pub fn into_path(self) -> PathBuf {
        self.path
    }
}

pub fn cache_digest(url: &RepositoryUrl) -> String {
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    let hash = hasher.finish();
    format!("{:x}", hash)
}
