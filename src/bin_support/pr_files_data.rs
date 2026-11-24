// Preview/test assembly while the Conversation host is landing.
#[path = "../data/mod.rs"]
mod base;
pub use base::*;
#[path = "../data/pr_files.rs"]
pub mod pr_files;

// The nested data module's existing tests also refer to these fixtures by
// their production crate path. Keep that path available in this example.
#[cfg(test)]
mod account_tests {
    use crate::domain::{AccountKey, HostId};
    use crate::network::transport::{HttpResponse, MockTransport};
    use crate::storage::accounts::AccountDatabaseConfig;

    pub(super) fn key(id: u64) -> AccountKey {
        AccountKey::new(id, HostId::github())
    }

    pub(super) async fn user(mock: &MockTransport, id: u64) {
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: format!(r#"{{"id":{id},"login":"user{id}"}}"#),
        }))
        .await;
    }

    pub(super) fn config(temp: &tempfile::TempDir) -> AccountDatabaseConfig {
        AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3"))
    }
}
