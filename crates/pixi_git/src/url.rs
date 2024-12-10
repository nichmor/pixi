use url::Url;

/// A wrapper around `Url` which represents a "canonical" version of an original URL.
///
/// A "canonical" url is only intended for internal comparison purposes. It's to help paper over
/// mistakes such as depending on `github.com/foo/bar` vs. `github.com/foo/bar.git`.
///
/// This is **only** for internal purposes and provides no means to actually read the underlying
/// string value of the `Url` it contains. This is intentional, because all fetching should still
/// happen within the context of the original URL.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub struct CanonicalUrl(Url);

impl CanonicalUrl {
    pub fn new(url: &Url) -> Self {
        let mut url = url.clone();

        // If the URL cannot be a base, then it's not a valid URL anyway.
        if url.cannot_be_a_base() {
            return Self(url);
        }

        // If the URL has no host, then it's not a valid URL anyway.
        if !url.has_host() {
            return Self(url);
        }

        // Strip credentials.
        let _ = url.set_password(None);
        let _ = url.set_username("");

        // Strip a trailing slash.
        if url.path().ends_with('/') {
            url.path_segments_mut().unwrap().pop_if_empty();
        }

        // For GitHub URLs specifically, just lower-case everything. GitHub
        // treats both the same, but they hash differently, and we're gonna be
        // hashing them. This wants a more general solution, and also we're
        // almost certainly not using the same case conversion rules that GitHub
        // does. (See issue #84)
        if url.host_str() == Some("github.com") {
            url.set_scheme(url.scheme().to_lowercase().as_str())
                .unwrap();
            let path = url.path().to_lowercase();
            url.set_path(&path);
        }

        // Repos can generally be accessed with or without `.git` extension.
        if let Some((prefix, suffix)) = url.path().rsplit_once('@') {
            // Ex) `git+https://github.com/pypa/sample-namespace-packages.git@2.0.0`
            let needs_chopping = std::path::Path::new(prefix)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("git"));
            if needs_chopping {
                let prefix = &prefix[..prefix.len() - 4];
                url.set_path(&format!("{prefix}@{suffix}"));
            }
        } else {
            // Ex) `git+https://github.com/pypa/sample-namespace-packages.git`
            let needs_chopping = std::path::Path::new(url.path())
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("git"));
            if needs_chopping {
                let last = {
                    let last = url.path_segments().unwrap().next_back().unwrap();
                    last[..last.len() - 4].to_owned()
                };
                url.path_segments_mut().unwrap().pop().push(&last);
            }
        }

        Self(url)
    }

    pub fn parse(url: &str) -> Result<Self, url::ParseError> {
        Ok(Self::new(&Url::parse(url)?))
    }
}

/// Like [`CanonicalUrl`], but attempts to represent an underlying source repository, abstracting
/// away details like the specific commit or branch, or the subdirectory to build within the
/// repository.
///
/// For example, `https://github.com/pypa/package.git#subdirectory=pkg_a` and
/// `https://github.com/pypa/package.git#subdirectory=pkg_b` would map to different
/// [`CanonicalUrl`] values, but the same [`RepositoryUrl`], since they map to the same
/// resource.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Hash)]
pub struct RepositoryUrl(Url);

impl RepositoryUrl {
    pub fn new(url: &Url) -> Self {
        let mut url = CanonicalUrl::new(url).0;

        // If a Git URL ends in a reference (like a branch, tag, or commit), remove it.
        if url.scheme().starts_with("git+") {
            if let Some(prefix) = url
                .path()
                .rsplit_once('@')
                .map(|(prefix, _suffix)| prefix.to_string())
            {
                url.set_path(&prefix);
            }
        }

        // Drop any fragments and query parameters.
        url.set_fragment(None);
        url.set_query(None);

        Self(url)
    }

    pub fn parse(url: &str) -> Result<Self, url::ParseError> {
        Ok(Self::new(&Url::parse(url)?))
    }
}
