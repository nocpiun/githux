use super::tests::{board, ready_for, target};
use super::*;
#[test]
fn performance_swr_validation_queued_before_expiry_recovers_without_leaking_old_terminal() {
    let mut board = board();
    let target = target("late.png");
    let key = target.key();
    board.want(0, target);
    board.take_pending(12);
    let old_request = board.requests[&key].0;
    let sink = board.request_sink(&key);
    let current = ImageValidity::new(|| true, None);
    sink.publish(
        board.scope(),
        key.clone(),
        0,
        false,
        Some(ImageValidity::new(
            || true,
            Some(std::time::Instant::now() + std::time::Duration::from_secs(60)),
        )),
        Ok(ImageUpdate::Ready(ready_for(&board, 6, 3).result.unwrap())),
    );
    for event in sink.drain_events() {
        board.apply_event(event);
    }
    assert!(board.is_ready(&key));
    sink.publish(
        board.scope(),
        key.clone(),
        1,
        false,
        Some(current.clone()),
        Ok(ImageUpdate::Validated),
    );
    sink.publish(
        board.scope(),
        key.clone(),
        2,
        true,
        Some(current.clone()),
        Ok(ImageUpdate::Finished),
    );
    board.validity.get_mut(&key).unwrap().expires =
        Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
    assert_eq!(board.revoke_invalid(), vec![0]);
    for event in sink.drain_events() {
        board.apply_event(event);
    }
    assert!(matches!(board.state(&key), Some(ImageState::Loading)));
    assert_eq!(board.in_flight, 0);
    assert_eq!(board.take_pending(12).len(), 1);
    assert_ne!(board.requests[&key].0, old_request);
    let fresh = board.request_sink(&key);
    fresh.publish(
        board.scope(),
        key.clone(),
        0,
        true,
        Some(current),
        Ok(ImageUpdate::Ready(ready_for(&board, 6, 3).result.unwrap())),
    );
    for event in fresh.drain_events() {
        board.apply_event(event);
    }
    assert!(board.is_ready(&key));
    assert_eq!(board.in_flight, 0);
    assert!(!board.work_outstanding());
}
#[test]
fn performance_swr_error_keeps_pixels_only_until_permitted_expiry() {
    let mut board = board();
    let target = target("stale.png");
    let key = target.key();
    board.want(1, target);
    board.take_pending(12);
    let sink = board.request_sink(&key);
    let validity = ImageValidity::new(
        || true,
        Some(std::time::Instant::now() + std::time::Duration::from_secs(60)),
    );
    let decoded = ready_for(&board, 6, 3).result.unwrap();
    sink.publish(
        board.scope(),
        key.clone(),
        0,
        false,
        Some(validity),
        Ok(ImageUpdate::Ready(decoded)),
    );
    sink.publish(
        board.scope(),
        key.clone(),
        1,
        true,
        None,
        Err(ImageFailure::Network),
    );
    for event in sink.drain_events() {
        board.apply_event(event);
    }
    assert!(board.states[&key].is_ready());
    assert!(board.work_outstanding());
    assert!(!board.active_work());
    assert!(board.next_wake() > std::time::Duration::from_secs(1));
    board.validity.get_mut(&key).unwrap().expires =
        Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
    assert_eq!(board.revoke_invalid(), vec![1]);
    assert!(!board.states[&key].is_ready());
    assert_eq!(board.ready_bytes, 0);
    assert!(!board.work_outstanding());
}

#[test]
fn performance_animation_finish_and_document_replacement_retire_only_old_atlas_images() {
    let mut board = board();
    let target = target("stats.svg");
    let key = target.key();
    board.want(1, target);
    board.take_pending(12);
    let decoded = crate::content::images::decode_image(
        include_bytes!("../../content/images/fixtures/entry-stats.svg").to_vec(),
        &ImageLimits::default(),
    )
    .unwrap();
    board.apply(ImageCompletion {
        scope: board.scope(),
        key: key.clone(),
        result: Ok(decoded),
    });
    let final_image = match &board.animations[&key].final_image {
        ImageSource::Render(image) => image.clone(),
        _ => panic!("predecoded final"),
    };
    let frame_count = board.animations[&key].frames.len();
    board.finish_animation(&key);
    assert_eq!(board.retired.len(), frame_count);
    assert!(!board.retired.contains_key(&final_image.id));
    assert_eq!(board.ready_bytes, final_image.as_bytes(0).unwrap().len());
    drop(board.take_retired());
    board.begin(2);
    assert!(board.retired.contains_key(&final_image.id));
    drop(board.take_retired());
    assert!(board.retired.is_empty());
}

use crate::content::images::{DecodedPixels, ImageUpdate};

fn observe(board: &mut ImageBoard, visible: std::ops::Range<usize>, width: f32) {
    board.begin_demand_frame();
    for i in 0..9 {
        board.observe_demand(
            target(&format!("{i}.png")).key(),
            if visible.contains(&i) { 2 } else { 0 },
            width,
        );
    }
    board.finish_demand_frame();
}
fn complete_pixels(board: &mut ImageBoard, request: ImageRequest, w: u32, h: u32) {
    let mut completion = ready_for(board, 2559, 1520);
    completion.key = request.key;
    let decoded = completion.result.as_mut().unwrap();
    decoded.pixels = Some(DecodedPixels {
        width: w,
        height: h,
        bgra: vec![0; w as usize * h as usize * 4],
    });
    board.apply(completion);
    // Legacy helper completes through apply; remove its request identity too.
    board.requests.clear();
    board.evict();
    drop(board.take_retired()); // Simulate the next window-side atlas cleanup.
}
#[test]
fn performance_nine_large_pictures_recover_only_after_viewport_demand_changes() {
    let mut board = board();
    board.manage_geometry();
    for i in 0..9 {
        board.want(i, target(&format!("{i}.png")));
    }
    observe(&mut board, 0..4, 2559.0);
    let requests = board.take_pending(12);
    assert_eq!(requests.len(), 4);
    for request in requests {
        complete_pixels(&mut board, request, 2559, 1520);
    }
    let four = 4 * 2559 * 1520 * 4;
    assert_eq!(board.ready_bytes, four);
    assert!(four < 64 * 1024 * 1024);
    observe(&mut board, 4..8, 2559.0);
    for request in board.take_pending(12) {
        complete_pixels(&mut board, request, 2559, 1520);
    }
    for i in 4..8 {
        assert!(board.states[&target(&format!("{i}.png")).key()].is_ready());
    }
    for _ in 0..10 {
        observe(&mut board, 4..8, 2559.0);
        assert!(board.take_pending(12).is_empty());
    }
    observe(&mut board, 0..4, 2559.0);
    let recovered = board.take_pending(12);
    assert_eq!(
        recovered.len(),
        4,
        "scrolling back recovers evicted originals"
    );
    for request in recovered {
        complete_pixels(&mut board, request, 2559, 1520);
    }
    for _ in 0..10 {
        observe(&mut board, 0..4, 2559.0);
        assert!(board.take_pending(12).is_empty());
    }
    assert_eq!(board.ready_bytes, four);
    assert_eq!(board.reloads, 4);
    assert_eq!(board.evictions, 8);
    eprintln!(
        "nine_native_probe: retained={} peak_pixels_during_replacement={} evictions={} reloads={}",
        board.ready_bytes, board.peak_pixel_bytes, board.evictions, board.reloads
    );
}
#[test]
fn performance_scope_change_keeps_original_dimensions_without_a_released_layout_box() {
    let mut board = board();
    board.manage_geometry();
    board.limits.max_ready_images = 0;
    for i in 0..9 {
        board.want(i, target(&format!("{i}.png")));
    }
    observe(&mut board, 0..1, 600.0);
    let request = board.take_pending(12).pop().unwrap();
    let key = request.key.clone();
    complete_pixels(&mut board, request, 600, 357);
    assert!(matches!(board.states[&key], ImageState::Released));
    assert!(!board.has_layout_geometry(&key));
    let size = board.intrinsic_size(&key);
    board.advance(board.scope().generation + 1);
    assert!(matches!(board.states[&key], ImageState::Released));
    assert_eq!(board.intrinsic_size(&key), size);
    assert!(!board.has_layout_geometry(&key));
    observe(&mut board, 0..1, 600.0);
    assert_eq!(board.take_pending(12).len(), 1);
    observe(&mut board, 0..1, 600.0);
    assert!(board.take_pending(12).is_empty());
}
#[test]
fn performance_static_raster_shrinks_after_layout_settles_without_small_width_churn() {
    let mut board = board();
    board.manage_geometry();
    for i in 0..9 {
        board.want(i, target(&format!("{i}.png")));
    }
    observe(&mut board, 0..1, 900.0);
    let request = board.take_pending(12).pop().unwrap();
    let key = request.key.clone();
    complete_pixels(&mut board, request, 900, 535);
    observe(&mut board, 0..1, 850.0);
    assert!(board.take_pending(12).is_empty());
    observe(&mut board, 0..1, 600.0);
    let request = board
        .take_pending(12)
        .pop()
        .expect("settled narrow layout refines oversized pixels");
    complete_pixels(&mut board, request, 600, 357);
    for width in [600.0, 599.0, 600.0] {
        observe(&mut board, 0..1, width);
        assert!(board.take_pending(12).is_empty());
    }
    assert_eq!(board.grid_debug(&key), Some(600));
    assert_eq!(board.intrinsic_size(&key), Some((2559, 1520)));
}
#[test]
fn performance_display_grid_stays_small_and_wider_column_or_dpi_recovers() {
    let mut board = board();
    board.manage_geometry();
    for i in 0..9 {
        board.want(i, target(&format!("{i}.png")));
    }
    observe(&mut board, 0..9, 600.0);
    for request in board.take_pending(12) {
        complete_pixels(&mut board, request, 600, 357);
    }
    assert_eq!(board.ready_bytes, 9 * 600 * 357 * 4);
    observe(&mut board, 0..9, 600.0);
    assert!(board.take_pending(12).is_empty());
    observe(&mut board, 0..2, 900.0);
    let wider = board.take_pending(12);
    assert_eq!(wider.len(), 2);
    for request in wider {
        complete_pixels(&mut board, request, 900, 535);
    }
    board.set_render_options(ImageRenderOptions::for_device_scale(2.0));
    observe(&mut board, 0..2, 900.0);
    let denser = board.take_pending(12);
    assert_eq!(denser.len(), 2);
    for request in denser {
        complete_pixels(&mut board, request, 1800, 1070);
    }
    observe(&mut board, 0..2, 900.0);
    assert!(board.take_pending(12).is_empty());
    assert_eq!(
        board.intrinsic_size(&target("0.png").key()),
        Some((2559, 1520))
    );
}
#[test]
fn performance_duplicate_out_of_order_and_stale_progressive_events_release_once() {
    let mut board = board();
    let target = target("one.png");
    let key = target.key();
    board.want(1, target);
    board.take_pending(12);
    let sink = board.request_sink(&key);
    let scope = board.scope();
    sink.publish(scope, key.clone(), 1, true, None, Ok(ImageUpdate::Finished));
    for event in sink.drain_events() {
        board.apply_event(event);
    }
    assert_eq!(
        board.in_flight, 1,
        "out of order terminal must not release admission"
    );
    let mut decoded = ready_for(&board, 6, 3).result.unwrap();
    decoded.pixels = Some(DecodedPixels {
        width: 6,
        height: 3,
        bgra: vec![0; 72],
    });
    sink.publish(
        scope,
        key.clone(),
        0,
        false,
        None,
        Ok(ImageUpdate::Ready(decoded)),
    );
    for event in sink.drain_events() {
        board.apply_event(event);
    }
    assert_eq!(board.in_flight, 1);
    sink.publish(scope, key.clone(), 1, true, None, Ok(ImageUpdate::Finished));
    sink.publish(scope, key.clone(), 1, true, None, Ok(ImageUpdate::Finished));
    for event in sink.drain_events() {
        board.apply_event(event);
    }
    assert_eq!(board.in_flight, 0);
    assert!(board.states[&key].is_ready());
    board.advance(2);
    sink.publish(scope, key, 2, true, None, Err(ImageFailure::Network));
    for event in sink.drain_events() {
        board.apply_event(event);
    }
    assert_eq!(board.in_flight, 0);
}
