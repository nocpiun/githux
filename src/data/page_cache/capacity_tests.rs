use super::{tests::*, *};
use crate::storage::cache::CacheConfig;

#[tokio::test]
async fn page_cache_storage_limits_preserve_successful_network_values_and_old_snapshot() {
    for entry_limit in [true, false] {
        let fixture = Fixture::new().await;
        let directory = tempfile::tempdir().unwrap();
        let mut config = CacheConfig::at(directory.path().to_path_buf());
        if entry_limit {
            config.max_entry_bytes = 1024;
            config.inline_bytes = 1024;
        } else {
            config.capacity_bytes = 8192;
            config.max_entry_bytes = 8192;
            config.inline_bytes = 8192;
        }
        let store = CacheStore::open(config).unwrap();
        let cached = fixture.cached(&store);
        let old_bio = if entry_limit {
            "old".into()
        } else {
            "b".repeat(2000)
        };
        fixture
            .enqueue(&serde_json::json!({"id":2,"login":"target","bio":old_bio}).to_string())
            .await;
        let (_, _, old_metadata) = value(
            cached
                .profile(PageReadMode::Online, "target", Default::default())
                .await
                .unwrap(),
        );
        // Keep the previous representation pinned to exercise capacity pressure
        // independently of the per-entry size limit.
        let key = cached
            .key(None, "profile-basic", serde_json::json!("target"), false)
            .unwrap();
        let CacheRead::Hit(_lease) = store.read(&cached.scope, &key).unwrap() else {
            panic!("old snapshot");
        };
        let _sibling = if !entry_limit {
            let sibling = cached
                .key(None, "capacity-sibling", serde_json::json!("other"), false)
                .unwrap();
            let ticket = store.begin_write(&cached.scope, &sibling).unwrap();
            store
                .put(
                    ticket,
                    &vec![0; 3000],
                    metadata(1, CacheCompleteness::Complete, false),
                )
                .unwrap();
            let CacheRead::Hit(lease) = store.read(&cached.scope, &sibling).unwrap() else {
                panic!("sibling snapshot");
            };
            Some(lease)
        } else {
            None
        };
        let large_bio = "a".repeat(if entry_limit { 2048 } else { 6000 });
        fixture
            .enqueue(&serde_json::json!({"id":2,"login":"target","bio":large_bio}).to_string())
            .await;
        let (profile, origin, metadata) = value(
            cached
                .profile(PageReadMode::Online, "target", Default::default())
                .await
                .unwrap(),
        );
        assert_eq!(origin, PageOrigin::Network);
        assert_eq!(profile.bio.as_deref(), Some(large_bio.as_str()));
        assert_eq!(metadata.completeness, CacheCompleteness::Complete);
        let (historical, origin, metadata) = value(
            cached
                .profile(PageReadMode::Offline, "target", Default::default())
                .await
                .unwrap(),
        );
        assert_eq!(origin, PageOrigin::HistoricalSnapshot);
        assert_eq!(historical.bio.as_deref(), Some(old_bio.as_str()));
        assert_eq!(metadata, old_metadata);
    }
}
