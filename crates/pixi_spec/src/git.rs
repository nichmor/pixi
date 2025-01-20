use std::fmt::Display;

use pixi_git::git::GitReference;
use serde::{Serialize, Serializer};
use thiserror::Error;
use url::Url;

/// A specification of a package from a git repository.
#[derive(Debug, Clone, Hash, Eq, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct GitSpec {
    /// The git url of the package which can contain git+ prefixes.
    pub git: Url,

    /// The git revision of the package
    #[serde(skip_serializing_if = "Reference::is_default_branch", flatten)]
    pub rev: Option<Reference>,

    /// The git subdirectory of the package
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subdirectory: Option<String>,
}

/// A reference to a specific commit in a git repository.
#[derive(Debug, Clone, Hash, Eq, PartialEq, PartialOrd, Ord, ::serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reference {
    /// The HEAD commit of a branch.
    Branch(String),

    /// A specific tag.
    Tag(String),

    /// A specific commit.
    Rev(String),

    /// A default branch.
    DefaultBranch,
}

impl Reference {
    /// Return the inner value
    pub fn reference(&self) -> Option<String> {
        match self {
            Reference::Branch(branch) => Some(branch.to_string()),
            Reference::Tag(tag) => Some(tag.to_string()),
            Reference::Rev(rev) => Some(rev.to_string()),
            Reference::DefaultBranch => None,
        }
    }

    /// Return if the reference is the default branch.
    pub fn is_default(&self) -> bool {
        matches!(self, Reference::DefaultBranch)
    }

    /// Returns the reference as a string.
    pub fn is_default_branch(reference: &Option<Reference>) -> bool {
        reference.is_none()
            || reference
                .as_ref()
                .is_some_and(|reference| matches!(reference, Reference::DefaultBranch))
    }

    /// Returns the full commit hash if possible.
    pub fn as_full_commit(&self) -> Option<&str> {
        match self {
            Reference::Rev(rev) => {
                GitReference::looks_like_full_commit_hash(rev).then_some(rev.as_str())
            }
            _ => None,
        }
    }
}

impl Display for Reference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Reference::Branch(branch) => write!(f, "{}", branch),
            Reference::Tag(tag) => write!(f, "{}", tag),
            Reference::Rev(rev) => write!(f, "{}", rev),
            Reference::DefaultBranch => write!(f, "HEAD"),
        }
    }
}

impl From<GitReference> for Reference {
    fn from(value: GitReference) -> Self {
        match value {
            GitReference::Branch(branch) => Reference::Branch(branch.to_string()),
            GitReference::Tag(tag) => Reference::Tag(tag.to_string()),
            GitReference::ShortCommit(rev) => Reference::Rev(rev.to_string()),
            GitReference::BranchOrTag(rev) => Reference::Rev(rev.to_string()),
            GitReference::BranchOrTagOrCommit(rev) => Reference::Rev(rev.to_string()),
            GitReference::NamedRef(rev) => Reference::Rev(rev.to_string()),
            GitReference::FullCommit(rev) => Reference::Rev(rev.to_string()),
            GitReference::DefaultBranch => Reference::DefaultBranch,
        }
    }
}

impl Serialize for Reference {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct RawReference<'a> {
            #[serde(skip_serializing_if = "Option::is_none")]
            tag: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            branch: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            rev: Option<&'a str>,
        }

        let ser = match self {
            Reference::Branch(name) => RawReference {
                branch: Some(name),
                tag: None,
                rev: None,
            },
            Reference::Tag(name) => RawReference {
                branch: None,
                tag: Some(name),
                rev: None,
            },
            Reference::Rev(name) => RawReference {
                branch: None,
                tag: None,
                rev: Some(name),
            },
            Reference::DefaultBranch => RawReference {
                branch: None,
                tag: None,
                rev: None,
            },
        };

        ser.serialize(serializer)
    }
}

#[derive(Error, Debug)]
/// An error that can occur when converting a `Reference` to a `GitReference`.
pub enum GitReferenceError {
    #[error("The commit string is invalid: \"{0}\"")]
    /// The commit string is invalid.
    InvalidCommit(String),
}

impl From<Reference> for GitReference {
    fn from(value: Reference) -> Self {
        match value {
            Reference::Branch(branch) => GitReference::Branch(branch),
            Reference::Tag(tag) => GitReference::Tag(tag),
            Reference::Rev(rev) => GitReference::from_rev(rev),
            Reference::DefaultBranch => GitReference::DefaultBranch,
        }
    }
}
