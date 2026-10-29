//! Reader picture state: bounds, generations and the address boundary.

use std::sync::Arc;

use super::*;
use crate::content::images::{DecodedImage, ImageCompletion, ImageFormat, ImageScope};

fn target(path: &str) -> ImageTarget {
    ImageTarget::Repository {
        owner: "o".into(),
        name: "n".into(),
        commit: "a".repeat(40),
        path: path.into(),
    }
}

/// A completion for the current scope of `board`, so only the fields each test
/// is about are set by the test itself.
fn ready_for(board: &ImageBoard, width: u32, height: u32) -> ImageCompletion {
    ImageCompletion {
        scope: board.scope(),
        key: ImageKey::new("unused"),
        result: Ok(DecodedImage {
            format: ImageFormat::Png,
            width,
            height,
            bytes: vec![0x89, b'P', b'N', b'G'],
        }),
    }
}

fn board() -> ImageBoard {
    let mut board = ImageBoard::new(ImageLimits::default());
    board.begin(1);
    board.set_attached(true);
    board
}

fn label(board: &ImageBoard, key: &ImageKey) -> Option<&'static str> {
    board.state(key).map(|state| state.label())
}

#[test]
fn a_source_is_requested_once_and_starts_in_the_loading_state() {
    let mut board = board();
    let first = board.want(3, target("docs/a.png"));
    assert_eq!(first.label(), "loading");
    let again = board.want(3, target("docs/a.png"));
    assert_eq!(again.label(), "loading");
    // Two blocks pointing at the same source share one request.
    let other_block = board.want(9, target("docs/a.png"));
    assert_eq!(other_block.label(), "loading");
    let requests = board.take_pending(10);
    assert_eq!(requests.len(), 1, "one request per source");
    assert_eq!(requests[0].scope.generation, 1);
    assert_eq!(requests[0].scope.reader, board.scope().reader);
    assert!(!board.has_pending());
}

#[test]
fn without_a_provider_a_source_is_unavailable_and_never_requested() {
    let mut board = ImageBoard::new(ImageLimits::default());
    board.begin(1);
    assert_eq!(
        board.want(0, target("docs/a.png")).label(),
        "no image service attached"
    );
    assert!(!board.has_pending());
    assert!(!board.work_outstanding());
    assert_eq!(board.known_states(), 0);
}

#[test]
fn the_in_flight_budget_bounds_outstanding_requests() {
    let limits = ImageLimits {
        max_in_flight: 2,
        ..ImageLimits::default()
    };
    let mut board = ImageBoard::new(limits);
    board.begin(1);
    board.set_attached(true);
    for index in 0..5 {
        board.want(index, target(&format!("docs/{index}.png")));
    }
    let first = board.take_pending(2);
    assert_eq!(first.len(), 2);
    assert_eq!(board.take_pending(2).len(), 0, "the budget is full");
    assert!(board.work_outstanding());
    // One completion frees a slot.
    let mut completion = ready_for(&board, 2, 2);
    completion.key = first[0].key.clone();
    board.apply(completion);
    assert_eq!(board.take_pending(2).len(), 1);
}

#[test]
fn a_completion_from_another_generation_is_dropped() {
    let mut board = board();
    assert_eq!(board.want(1, target("docs/a.png")).label(), "loading");
    let mut completion = ready_for(&board, 4, 4);
    completion.scope = ImageScope {
        reader: board.scope().reader,
        generation: board.scope().generation.wrapping_add(98),
    };
    completion.key = target("docs/a.png").key();
    assert!(board.apply(completion).is_empty());
    assert_eq!(
        label(&board, &target("docs/a.png").key()),
        Some("loading"),
        "the current generation keeps waiting"
    );
    // Another reader is a different scope even at the same generation number.
    let mut other_reader = ready_for(&board, 4, 4);
    other_reader.scope = ImageScope {
        reader: board.scope().reader.wrapping_add(1_000),
        generation: board.scope().generation,
    };
    other_reader.key = target("docs/a.png").key();
    assert!(
        board.apply(other_reader).is_empty(),
        "another reader's completion never lands here"
    );
}

#[test]
fn a_completion_for_a_dropped_source_is_ignored() {
    let mut board = board();
    board.want(1, target("docs/a.png"));
    board.begin(2);
    board.set_attached(true);
    let mut completion = ready_for(&board, 4, 4);
    completion.key = target("docs/a.png").key();
    assert!(board.apply(completion).is_empty());
    assert_eq!(board.known_states(), 0);
}

#[test]
fn a_ready_picture_remeasures_the_blocks_that_want_it() {
    let mut board = board();
    board.want(2, target("docs/a.png"));
    board.want(7, target("docs/a.png"));
    board.take_pending(10);
    let mut completion = ready_for(&board, 8, 4);
    completion.key = target("docs/a.png").key();
    let mut blocks = board.apply(completion);
    blocks.sort_unstable();
    assert_eq!(blocks, vec![2, 7]);
    assert_eq!(label(&board, &target("docs/a.png").key()), Some("ready"));
}

#[test]
fn the_decoded_budget_releases_the_oldest_picture() {
    let limits = ImageLimits {
        max_ready_images: 1,
        ..ImageLimits::default()
    };
    let mut board = ImageBoard::new(limits);
    board.begin(1);
    board.set_attached(true);
    for (index, name) in [(0, "a.png"), (1, "b.png")] {
        board.want(index, target(&format!("docs/{name}")));
        board.take_pending(10);
        let mut completion = ready_for(&board, 10, 10);
        completion.key = target(&format!("docs/{name}")).key();
        board.apply(completion);
    }
    board.evict();
    assert_eq!(
        label(&board, &target("docs/a.png").key()),
        Some("released"),
        "the oldest decoded picture is released"
    );
    assert_eq!(label(&board, &target("docs/b.png").key()), Some("ready"));
}

#[test]
fn suspending_keeps_ready_pictures_and_drops_the_rest() {
    let mut board = board();
    board.want(0, target("docs/a.png"));
    board.want(1, target("docs/b.png"));
    board.take_pending(10);
    let mut completion = ready_for(&board, 4, 4);
    completion.key = target("docs/a.png").key();
    board.apply(completion);
    assert!(board.work_outstanding(), "b.png is still owed");
    board.suspend();
    assert!(!board.work_outstanding());
    assert_eq!(label(&board, &target("docs/a.png").key()), Some("ready"));
    assert!(board.state(&target("docs/b.png").key()).is_none());
    // A visible frame asks again.
    assert_eq!(board.want(1, target("docs/b.png")).label(), "loading");
    assert!(board.has_pending());
}

#[test]
fn resolving_a_picture_address_uses_the_link_boundary() {
    let commit = "a".repeat(40);
    let context = crate::ui::reader::ResourceContext::new(
        "nocpiun",
        "githux",
        commit.clone(),
        "docs/README.md",
    );
    assert_eq!(
        resolve_target("images/a.png", &context),
        Ok(ImageTarget::Repository {
            owner: "nocpiun".into(),
            name: "githux".into(),
            commit: commit.clone(),
            path: "docs/images/a.png".into(),
        })
    );
    assert_eq!(
        resolve_target("/images/a.png", &context),
        Ok(ImageTarget::Repository {
            owner: "nocpiun".into(),
            name: "githux".into(),
            commit: commit.clone(),
            path: "images/a.png".into(),
        })
    );
    assert_eq!(
        resolve_target("https://example.com/a.png", &context),
        Ok(ImageTarget::External {
            url: "https://example.com/a.png".into(),
        })
    );
    // External pictures are HTTPS only, and local schemes never resolve.
    assert_eq!(
        resolve_target("http://example.com/a.png", &context),
        Err(ImageFailure::Refused(
            crate::content::images::ImageRefusal::InsecureExternal
        ))
    );
    for url in [
        "file:///etc/passwd",
        "data:image/png;base64,AAAA",
        "javascript:alert(1)",
        "//host/a.png",
        r"\\server\share\a.png",
        "#fragment",
        "",
    ] {
        assert!(
            matches!(resolve_target(url, &context), Err(ImageFailure::Refused(_))),
            "{url} must be refused"
        );
    }
}

/// The same path at two commits is two sources, so one document's picture can
/// never satisfy another's.
#[test]
fn the_same_path_at_two_commits_is_two_sources() {
    let context_a = crate::ui::reader::ResourceContext::new("o", "n", "a".repeat(40), "README.md");
    let context_b = crate::ui::reader::ResourceContext::new("o", "n", "b".repeat(40), "README.md");
    let a = resolve_target("images/x.png", &context_a).expect("a");
    let b = resolve_target("images/x.png", &context_b).expect("b");
    assert_ne!(a.key(), b.key());
}

#[test]
fn a_key_is_built_from_the_resolved_target_only() {
    let target = ImageTarget::Repository {
        owner: "o".into(),
        name: "n".into(),
        commit: "a".repeat(40),
        path: "docs/a.png".into(),
    };
    assert_eq!(
        target.key().as_str(),
        format!("repo:o/n@{}:docs/a.png", "a".repeat(40))
    );
    // A picture's identity is the resolved source; there is no credential field
    // through which a PAT could enter a key.
    let _keep: Arc<()> = Arc::new(());
}

#[test]
fn a_picture_of_every_state_draws_something() {
    // The element builder is exercised through the real renderer in the view
    // tests; this only checks the state labels the placeholders use.
    for state in [
        ImageState::Unavailable,
        ImageState::Released,
        ImageState::Failed(ImageFailure::NotFound),
        ImageState::Failed(ImageFailure::Malformed),
    ] {
        assert!(!state.is_ready());
        assert!(!state.label().is_empty());
    }
}

/// Finding: a quote/list/footnote can hold several distinct pictures under one
/// top-level block index. Each must still be found when it completes.
#[test]
fn every_picture_in_one_block_is_tracked() {
    let mut board = board();
    board.want(4, target("docs/first.png"));
    board.want(4, target("docs/second.png"));
    board.take_pending(10);

    let mut first = ready_for(&board, 4, 4);
    first.key = target("docs/first.png").key();
    assert_eq!(
        board.apply(first),
        vec![4],
        "the first picture still remeasures its owning block"
    );

    let mut second = ready_for(&board, 4, 4);
    second.key = target("docs/second.png").key();
    assert_eq!(
        board.apply(second),
        vec![4],
        "the second picture under the same block is not lost"
    );
    // One block appears once per key, not once per `want` call in a frame.
    board.want(4, target("docs/first.png"));
    let keys = board.blocks_for_test();
    assert_eq!(keys.get(&target("docs/first.png").key()), Some(&vec![4]));
}

/// Finding: replacing an attached provider must drop the old unfinished states
/// and ask the new provider again.
#[test]
fn forgetting_unfinished_work_keeps_ready_pictures_and_redemands_the_rest() {
    let mut board = board();
    board.want(0, target("docs/ready.png"));
    board.want(1, target("docs/pending.png"));
    board.take_pending(10);
    let mut completion = ready_for(&board, 4, 4);
    completion.key = target("docs/ready.png").key();
    board.apply(completion);
    assert!(board.work_outstanding(), "one picture is still owed");

    board.forget_unfinished();
    assert!(!board.work_outstanding());
    assert_eq!(
        label(&board, &target("docs/ready.png").key()),
        Some("ready"),
        "a finished picture is kept"
    );
    assert!(
        board.state(&target("docs/pending.png").key()).is_none(),
        "an unfinished picture is forgotten rather than stuck loading"
    );
    // The next frame asks again, so a new provider receives the request.
    assert_eq!(board.want(1, target("docs/pending.png")).label(), "loading");
    assert!(board.has_pending());
}

/// Finding: a stale completion that arrives after a generation change must not
/// consume a slot belonging to the new generation's in-flight work.
#[test]
fn a_stale_completion_does_not_consume_a_current_slot() {
    let mut board = board();
    board.want(0, target("docs/a.png"));
    let first_scope = board.scope();
    board.take_pending(10);

    // The document changes while the old completion is still queued.
    board.begin(first_scope.generation.wrapping_add(1));
    board.set_attached(true);
    board.want(0, target("docs/a.png"));
    assert_eq!(
        board.take_pending(10).len(),
        1,
        "the new picture is in flight"
    );
    assert!(board.work_outstanding());

    // The old completion arrives now.
    let mut stale = ready_for(&board, 4, 4);
    stale.scope = first_scope;
    stale.key = target("docs/a.png").key();
    assert!(board.apply(stale).is_empty());
    assert!(
        board.work_outstanding(),
        "the live request is still counted as outstanding"
    );

    // Its own completion is still applied and still counts.
    let mut fresh = ready_for(&board, 4, 4);
    fresh.key = target("docs/a.png").key();
    assert_eq!(board.apply(fresh), vec![0]);
    assert!(!board.work_outstanding());
    assert_eq!(label(&board, &target("docs/a.png").key()), Some("ready"));
}

/// Finding: eviction replaces a ready picture with a shorter placeholder, so the
/// owning block must be measured again.
#[test]
fn eviction_reports_the_blocks_it_shrank() {
    let limits = ImageLimits {
        max_ready_images: 1,
        ..ImageLimits::default()
    };
    let mut board = ImageBoard::new(limits);
    board.begin(1);
    board.set_attached(true);
    for (block, name) in [(3, "a.png"), (8, "b.png")] {
        board.want(block, target(&format!("docs/{name}")));
        board.take_pending(10);
        let mut completion = ready_for(&board, 10, 10);
        completion.key = target(&format!("docs/{name}")).key();
        board.apply(completion);
    }
    // Both blocks are registered under their own picture; the first is evicted.
    let mut affected = board.evict();
    affected.sort_unstable();
    assert_eq!(
        affected,
        vec![3],
        "the evicted picture's block is measured again"
    );
    assert_eq!(label(&board, &target("docs/a.png").key()), Some("released"));
    assert!(board.evict().is_empty(), "nothing left to evict");
}
