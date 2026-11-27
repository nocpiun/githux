use super::*;
use crate::data::page_cache::{
    PageOrigin,
    tests::{Fixture, temp, value},
};
use crate::domain::repository::RepositoryLocator;
use crate::network::transport::json_response;

fn pull(number: u64, title: &str) -> serde_json::Value {
    serde_json::json!({
        "id": 9000 + number,
        "number": number,
        "title": title,
        "state": "open",
        "draft": false,
        "merged_at": null,
        "user": {"login": "octo"},
        "head": {"ref": "feature/x"},
        "base": {"ref": "main"},
        "created_at": "2026-11-01T00:00:00Z",
        "updated_at": "2026-11-20T00:00:00Z"
    })
}

fn page_response(
    repository_id: u64,
    pulls: serde_json::Value,
    next: Option<u32>,
) -> crate::network::transport::HttpResponse {
    page_response_with(repository_id, pulls, next, 30)
}

fn page_response_with(
    repository_id: u64,
    pulls: serde_json::Value,
    next: Option<u32>,
    per_page: u32,
) -> crate::network::transport::HttpResponse {
    let mut response = json_response(200, &pulls.to_string());
    if let Some(next) = next {
        response.headers.push((
            "link".into(),
            format!(
                "<https://api.github.com/repositories/{repository_id}/pulls?state=open&per_page={per_page}&page={next}>; rel=\"next\""
            ),
        ));
    }
    response
}

fn page_rows(start: u64, count: u64) -> serde_json::Value {
    serde_json::Value::Array(
        (0..count)
            .map(|offset| pull(start + offset, "Restored row"))
            .collect(),
    )
}

fn identity(name: &str, id: u64) -> RepositoryIdentity {
    RepositoryIdentity {
        id,
        locator: RepositoryLocator::new("octo", name).unwrap(),
    }
}

#[tokio::test]
async fn every_page_actually_read_is_restored_under_its_first_page_generation() {
    let fixture = Fixture::new().await;
    let (_directory, store) = temp();
    let cached = fixture.cached(&store);
    let limits = PullListLimits::default();
    let repository = identity("first", 12);
    let collection = new_collection_generation();
    fixture
        .mock
        .enqueue(Ok(page_response(
            12,
            serde_json::json!([pull(42, "Page one"), pull(41, "Page one second row")]),
            Some(2),
        )))
        .await;
    let first = cached
        .repository_pulls_online(
            &repository,
            PullFilter::Open,
            &limits,
            None,
            Some(collection),
            CancellationToken::default(),
        )
        .await
        .unwrap();
    assert_eq!(first.pulls.len(), 2);

    fixture
        .mock
        .enqueue(Ok(page_response(
            12,
            serde_json::json!([pull(40, "Only on page two")]),
            None,
        )))
        .await;
    let second = cached
        .repository_pulls_online(
            &repository,
            PullFilter::Open,
            &limits,
            first.next.as_ref(),
            Some(collection),
            CancellationToken::default(),
        )
        .await
        .unwrap();
    assert_eq!(second.pulls[0].number, 40);

    let (restored, generation) = cached
        .repository_pulls_restore(
            &repository,
            PullFilter::Open,
            &limits,
            CancellationToken::default(),
        )
        .await
        .expect("the visited pages are restored");
    assert_eq!(restored.len(), 2);
    assert_eq!(restored[0].pulls.len(), 2);
    assert_eq!(restored[1].pulls[0].number, 40);
    assert!(restored[1].next.is_none());
    assert_eq!(generation, Some(collection));
}

#[tokio::test]
async fn a_replaced_first_page_does_not_offer_pages_from_the_old_generation() {
    let fixture = Fixture::new().await;
    let (_directory, store) = temp();
    let cached = fixture.cached(&store);
    let limits = PullListLimits::default();
    let repository = identity("first", 12);
    fixture
        .mock
        .enqueue(Ok(page_response(
            12,
            serde_json::json!([pull(42, "Old page one")]),
            Some(2),
        )))
        .await;
    let old_collection = new_collection_generation();
    let old_first = cached
        .repository_pulls_online(
            &repository,
            PullFilter::Open,
            &limits,
            None,
            Some(old_collection),
            CancellationToken::default(),
        )
        .await
        .unwrap();
    fixture
        .mock
        .enqueue(Ok(page_response(
            12,
            serde_json::json!([pull(40, "Old page two")]),
            None,
        )))
        .await;
    cached
        .repository_pulls_online(
            &repository,
            PullFilter::Open,
            &limits,
            old_first.next.as_ref(),
            Some(old_collection),
            CancellationToken::default(),
        )
        .await
        .unwrap();

    // A refresh replaces the first page with a new generation.
    let new_collection = new_collection_generation();
    fixture
        .mock
        .enqueue(Ok(page_response(
            12,
            serde_json::json!([pull(50, "New page one")]),
            Some(2),
        )))
        .await;
    cached
        .repository_pulls_online(
            &repository,
            PullFilter::Open,
            &limits,
            None,
            Some(new_collection),
            CancellationToken::default(),
        )
        .await
        .unwrap();

    // The old continuation is not part of the new generation, so the restored prefix stops at
    // the new first page instead of appending rows from the previous list.
    let (restored, generation) = cached
        .repository_pulls_restore(
            &repository,
            PullFilter::Open,
            &limits,
            CancellationToken::default(),
        )
        .await
        .expect("the current first page is restored");
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].pulls[0].number, 50);
    assert_eq!(generation, Some(new_collection));
}

#[tokio::test]
async fn every_partition_of_the_list_is_independent() {
    let fixture = Fixture::new().await;
    let (_directory, store) = temp();
    let cached = fixture.cached(&store);
    let limits = PullListLimits::default();
    let first = identity("first", 12);
    let second = identity("second", 13);
    fixture
        .mock
        .enqueue(Ok(page_response(
            12,
            serde_json::json!([pull(42, "Cached first page")]),
            None,
        )))
        .await;
    cached
        .repository_pulls_online(
            &first,
            PullFilter::Open,
            &limits,
            None,
            None,
            CancellationToken::default(),
        )
        .await
        .unwrap();

    // The other filter, the other numeric repository and another page-size representation are
    // different list identities, not hits for this one.
    for (repository, filter) in [(&second, PullFilter::Open), (&first, PullFilter::Closed)] {
        assert!(matches!(
            cached
                .repository_pulls(
                    PageReadMode::Offline,
                    repository,
                    filter,
                    &limits,
                    None,
                    CancellationToken::default()
                )
                .await
                .unwrap()
                .value,
            PageValue::Miss(_)
        ));
    }
    let other_representation = PullListLimits {
        per_page: 50,
        ..PullListLimits::default()
    };
    assert!(matches!(
        cached
            .repository_pulls(
                PageReadMode::Offline,
                &first,
                PullFilter::Open,
                &other_representation,
                None,
                CancellationToken::default()
            )
            .await
            .unwrap()
            .value,
        PageValue::Miss(_)
    ));

    // A second saved account is a different private partition even for the same query.
    fixture
        .mock
        .enqueue(Ok(json_response(200, r#"{"id":2,"login":"other"}"#)))
        .await;
    fixture
        .service
        .login(crate::auth::token::Pat::from_str(
            "synthetic-second-account",
        ))
        .await
        .unwrap();
    let other = fixture.cached(&store);
    assert!(matches!(
        other
            .repository_pulls(
                PageReadMode::Offline,
                &first,
                PullFilter::Open,
                &limits,
                None,
                CancellationToken::default()
            )
            .await
            .unwrap()
            .value,
        PageValue::Miss(_)
    ));
    assert!(!cached.verify());
}

#[tokio::test]
async fn a_partial_page_stops_the_restored_prefix_and_no_store_invalidates_it() {
    let fixture = Fixture::new().await;
    let (_directory, store) = temp();
    let cached = fixture.cached(&store);
    let limits = PullListLimits::default();
    let repository = identity("first", 12);
    let mut partial = page_response(12, serde_json::json!([pull(9, "Partial page")]), Some(2));
    partial.headers.push((
        "x-github-sso".into(),
        "partial-results; organizations=acme".into(),
    ));
    fixture.mock.enqueue(Ok(partial)).await;
    cached
        .repository_pulls_online(
            &repository,
            PullFilter::Closed,
            &limits,
            None,
            None,
            CancellationToken::default(),
        )
        .await
        .unwrap();
    let (restored, _) = cached
        .repository_pulls_restore(
            &repository,
            PullFilter::Closed,
            &limits,
            CancellationToken::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        restored.len(),
        1,
        "a partial page's continuation is not followed"
    );
    assert_eq!(restored[0].pulls.len(), 1);
    assert!(matches!(
        restored[0].completeness,
        Completeness::Partial { .. }
    ));

    // A no-store answer invalidates the stored representation instead of leaving a replayable
    // snapshot behind.
    let mut no_store = page_response(12, serde_json::json!([pull(8, "No-store page")]), None);
    no_store
        .headers
        .push(("cache-control".into(), "no-store".into()));
    fixture.mock.enqueue(Ok(no_store)).await;
    cached
        .repository_pulls_online(
            &repository,
            PullFilter::Closed,
            &limits,
            None,
            None,
            CancellationToken::default(),
        )
        .await
        .unwrap();
    assert!(
        cached
            .repository_pulls_restore(
                &repository,
                PullFilter::Closed,
                &limits,
                CancellationToken::default()
            )
            .await
            .is_none()
    );
}

#[tokio::test]
async fn a_transient_refresh_failure_keeps_the_visited_pages_replayable() {
    let fixture = Fixture::new().await;
    let (_directory, store) = temp();
    let cached = fixture.cached(&store);
    let limits = PullListLimits::default();
    let repository = identity("first", 12);
    fixture
        .mock
        .enqueue(Ok(page_response(
            12,
            serde_json::json!([pull(42, "First repo")]),
            Some(2),
        )))
        .await;
    let collection = new_collection_generation();
    let first = cached
        .repository_pulls_online(
            &repository,
            PullFilter::Open,
            &limits,
            None,
            Some(collection),
            CancellationToken::default(),
        )
        .await
        .unwrap();
    fixture
        .mock
        .enqueue(Ok(page_response(
            12,
            serde_json::json!([pull(40, "Second page")]),
            None,
        )))
        .await;
    cached
        .repository_pulls_online(
            &repository,
            PullFilter::Open,
            &limits,
            first.next.as_ref(),
            Some(collection),
            CancellationToken::default(),
        )
        .await
        .unwrap();

    fixture.mock.enqueue(Ok(json_response(503, "{}"))).await;
    assert!(
        cached
            .repository_pulls_online(
                &repository,
                PullFilter::Open,
                &limits,
                None,
                Some(new_collection_generation()),
                CancellationToken::default()
            )
            .await
            .is_err()
    );

    let (restored, generation) = cached
        .repository_pulls_restore(
            &repository,
            PullFilter::Open,
            &limits,
            CancellationToken::default(),
        )
        .await
        .expect("the visited pages survive a failed refresh");
    assert_eq!(restored.len(), 2);
    assert_eq!(restored[1].pulls[0].number, 40);
    assert_eq!(generation, Some(collection));
    let (_, origin, _) = {
        let read = cached
            .repository_pulls(
                PageReadMode::Offline,
                &repository,
                PullFilter::Open,
                &limits,
                None,
                CancellationToken::default(),
            )
            .await
            .unwrap();
        value(read)
    };
    assert_eq!(origin, PageOrigin::HistoricalSnapshot);
}

#[tokio::test]
async fn a_page_that_would_cross_the_row_budget_is_not_restored() {
    let fixture = Fixture::new().await;
    let (_directory, store) = temp();
    let cached = fixture.cached(&store);
    let limits = PullListLimits {
        per_page: 75,
        ..PullListLimits::default()
    };
    let repository = identity("first", 12);
    let collection = new_collection_generation();
    // Three legal pages of 75 rows each = 225 rows; the restore budget is 200.
    let mut next = None;
    for (page, first) in [(1u32, 100u64), (2, 200), (3, 300)] {
        fixture
            .mock
            .enqueue(Ok(page_response_with(
                12,
                page_rows(first, 75),
                (page < 3).then_some(page + 1),
                limits.per_page,
            )))
            .await;
        let response = cached
            .repository_pulls_online(
                &repository,
                PullFilter::Open,
                &limits,
                next.as_ref(),
                Some(collection),
                CancellationToken::default(),
            )
            .await
            .unwrap();
        next = response.next;
    }

    let (restored, generation) = cached
        .repository_pulls_restore(
            &repository,
            PullFilter::Open,
            &limits,
            CancellationToken::default(),
        )
        .await
        .expect("the visited prefix is restored");
    assert_eq!(generation, Some(collection));
    assert_eq!(restored.len(), 2, "150 rows are inside the 200-row budget");
    assert_eq!(
        restored.iter().map(|page| page.pulls.len()).sum::<usize>(),
        150
    );
    assert_eq!(restored[0].pulls[0].number, 100);
    assert_eq!(restored[1].pulls[0].number, 200);
    // The page that would have crossed the budget is not restored, and the last restored page
    // still carries its real continuation marker: the prefix is honestly partial.
    assert!(restored[1].next.is_some());
    assert!(
        restored
            .iter()
            .all(|page| page.pulls.iter().all(|row| row.number < 300))
    );
}
