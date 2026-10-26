//! Fixed-version file identities derived from existing snapshot observations.

use crate::domain::repository::{
    DirectoryTree, ObjectSha, ReadmeDescriptor, RepositoryError, RepositoryIdentity,
    RepositoryPath, TreeEntryKind,
};

/// Carries path and commit for future relative-resource resolution. It is not
/// an account/authorization key and contains no URL or credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobIdentity {
    repository: RepositoryIdentity,
    commit: ObjectSha,
    path: RepositoryPath,
    blob: ObjectSha,
    size: Option<u64>,
}

impl BlobIdentity {
    pub fn from_file(tree: &DirectoryTree, index: usize) -> Result<Self, RepositoryError> {
        let entry = tree
            .entries
            .get(index)
            .ok_or(RepositoryError::InvalidInput {
                field: "tree index",
            })?;
        if !matches!(entry.kind, TreeEntryKind::File { .. }) {
            return Err(RepositoryError::InvalidInput {
                field: "ordinary file entry",
            });
        }
        let path = match tree.directory.path() {
            Some(parent) => {
                RepositoryPath::new(format!("{}/{}", parent.as_str(), entry.path.as_str()))?
            }
            None => entry.path.clone(),
        };
        Ok(Self {
            repository: tree.directory.repository().clone(),
            commit: tree.directory.commit().clone(),
            path,
            blob: entry.sha.clone(),
            size: entry.size,
        })
    }

    pub fn from_readme(readme: &ReadmeDescriptor) -> Self {
        Self {
            repository: readme.repository.clone(),
            commit: readme.commit.clone(),
            path: readme.path.clone(),
            blob: readme.blob.clone(),
            size: Some(readme.size),
        }
    }

    pub fn repository(&self) -> &RepositoryIdentity {
        &self.repository
    }
    pub fn commit(&self) -> &ObjectSha {
        &self.commit
    }
    pub fn path(&self) -> &RepositoryPath {
        &self.path
    }
    pub fn blob(&self) -> &ObjectSha {
        &self.blob
    }
    pub fn size(&self) -> Option<u64> {
        self.size
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlobLimits {
    /// Decoded body budget; default 2 MiB is an engineering limit.
    pub decoded_bytes: usize,
    /// Receive-time JSON budget, including base64 and metadata.
    pub response_bytes: usize,
}

impl Default for BlobLimits {
    fn default() -> Self {
        Self {
            decoded_bytes: 2 * 1024 * 1024,
            response_bytes: 8 * 1024 * 1024,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum BlobBody {
    Bytes(Vec<u8>),
    TooLarge { size: u64, limit: usize },
}

/// Constructed only after validation or an explicit size rejection. No token,
/// response JSON, download URL or duplicate base64 representation is retained.
#[derive(Debug, PartialEq, Eq)]
pub struct BlobRead {
    pub(crate) identity: BlobIdentity,
    pub(crate) body: BlobBody,
}

impl BlobRead {
    pub fn identity(&self) -> &BlobIdentity {
        &self.identity
    }
    pub fn body(&self) -> &BlobBody {
        &self.body
    }
    pub fn into_parts(self) -> (BlobIdentity, BlobBody) {
        (self.identity, self.body)
    }
}
