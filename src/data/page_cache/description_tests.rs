use super::{tests::*, *};
use crate::github::home::HomeSearchLimits;

/// A description batch is a caller-supplied list of coordinates. Its cache
/// identity must stay within `CacheKey`'s 4 KiB request budget, otherwise a
/// perfectly valid online read fails before any lookup runs.
#[tokio::test]
async fn large_description_batches_keep_a_bounded_cache_identity() {
    let fixture = Fixture::new().await;
    let (_directory, store) = temp();
    let cached = fixture.cached(&store);

    let batch: Vec<(String, String)> = (0..200)
        .map(|index| {
            (
                format!("owner-{index:03}-{}", "x".repeat(40)),
                format!("repo-{index:03}-{}", "y".repeat(40)),
            )
        })
        .collect();
    // Guard the fixture itself: the pre-fix identity (whole vector) really did
    // exceed CacheKey's 4 KiB request budget.
    assert!(serde_json::json!([batch.clone(), 1]).to_string().len() > 4 * 1024);
    let (owner, repository) = batch[0].clone();
    let key = format!("{owner}/{repository}").to_ascii_lowercase();
    fixture
        .enqueue(
            &serde_json::json!({
                "id": 7,
                "name": repository,
                "owner": {"id": 3, "login": owner},
                "visibility": "public",
                "private": false,
                "default_branch": "main",
                "fork": false,
                "archived": false,
                "stargazers_count": 0,
                "forks_count": 0,
                "open_issues_count": 0,
                "size": 1,
                "description": "bounded description",
                "html_url": "https://github.com/example/example"
            })
            .to_string(),
        )
        .await;

    let before = fixture.mock.recorded_requests().await.len();
    // One lookup keeps the mock deterministic; the identity bug triggered even
    // with a single lookup because the whole vector used to be embedded.
    let (map, origin, metadata) = value(
        cached
            .repository_descriptions(PageReadMode::Online, batch.clone(), 1, Default::default())
            .await
            .expect("a valid large batch must not fail on the cache key budget"),
    );
    assert_eq!(origin, PageOrigin::Network);
    assert_eq!(
        map.get(&key),
        Some(&Some("bounded description".to_string()))
    );
    assert!(
        matches!(metadata.completeness, CacheCompleteness::Partial { .. }),
        "coordinates beyond the lookup budget stay unresolved, not complete"
    );
    assert_eq!(
        fixture.mock.recorded_requests().await.len() - before,
        1,
        "the lookup budget still bounds requests"
    );

    let (offline, origin, _) = value(
        cached
            .repository_descriptions(PageReadMode::Offline, batch.clone(), 1, Default::default())
            .await
            .unwrap(),
    );
    assert_eq!(origin, PageOrigin::HistoricalSnapshot);
    assert_eq!(
        offline.get(&key),
        Some(&Some("bounded description".to_string()))
    );

    assert_eq!(offline, map);
    // Unresolved tail coordinates still belong to the result identity. Neither
    // changing only that tail, changing order, nor changing the budget may
    // replay the snapshot of this batch.
    let mut different_tail = batch.clone();
    different_tail[199] = ("someone".into(), "else".into());
    let mut different_order = batch.clone();
    different_order.swap(0, 1);
    for (input, budget) in [
        (different_tail, 1),
        (different_order, 1),
        (batch.clone(), 0),
    ] {
        assert!(matches!(
            cached
                .repository_descriptions(PageReadMode::Offline, input, budget, Default::default())
                .await
                .unwrap()
                .value,
            PageValue::Miss(_)
        ));
    }
    let before = fixture.mock.recorded_requests().await.len();
    let (zero_budget, _, _) = value(
        cached
            .repository_descriptions(PageReadMode::Online, batch.clone(), 0, Default::default())
            .await
            .unwrap(),
    );
    assert_eq!(zero_budget.len(), batch.len());
    assert!(zero_budget.values().all(Option::is_none));
    assert_eq!(fixture.mock.recorded_requests().await.len(), before);
    let (offline_zero, _, _) = value(
        cached
            .repository_descriptions(PageReadMode::Offline, batch, 0, Default::default())
            .await
            .unwrap(),
    );
    assert_eq!(offline_zero, zero_budget);

    // Sanity: the Home aggregate key still fits the same budget for the
    // default limits, so this fix did not trade one overflow for another.
    cached
        .home_keys(None, &HomeSearchLimits::default())
        .unwrap();
}
