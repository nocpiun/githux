#[test]
fn performance_phase_allocation_and_terminal_enqueue_are_serialized() {
    use std::sync::{atomic::AtomicU32, mpsc};
    use std::time::Duration;
    let sink = ImageSink::new().for_request(17);
    let phase = Arc::new(AtomicU32::new(0));
    let scope = ImageScope {
        reader: 1,
        generation: 1,
    };
    let key = ImageKey::new("timeout-race");
    let (entered, at_allocation) = mpsc::sync_channel(1);
    let (release, paused) = mpsc::sync_channel(1);
    let worker_sink = sink.clone();
    let worker_phase = phase.clone();
    let worker_key = key.clone();
    let worker = std::thread::spawn(move || {
        worker_sink.publish_with(
            scope,
            worker_key,
            || {
                let next = worker_phase.fetch_add(1, Ordering::Relaxed);
                entered.send(()).unwrap();
                paused.recv_timeout(Duration::from_secs(5)).unwrap();
                next
            },
            false,
            None,
            Ok(ImageUpdate::Finished),
        );
    });
    at_allocation.recv_timeout(Duration::from_secs(5)).unwrap();
    let (started, terminal_started) = mpsc::sync_channel(1);
    let (finished, terminal_finished) = mpsc::sync_channel(1);
    let terminal_sink = sink.clone();
    let terminal_phase = phase.clone();
    let terminal_key = key.clone();
    let timeout = std::thread::spawn(move || {
        started.send(()).unwrap();
        terminal_sink.publish_next(
            scope,
            terminal_key,
            &terminal_phase,
            true,
            None,
            Err(ImageFailure::Timeout),
        );
        finished.send(()).unwrap();
    });
    terminal_started
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    let terminal_overtook_worker = terminal_finished
        .recv_timeout(Duration::from_millis(200))
        .is_ok();
    release.send(()).unwrap();
    worker.join().unwrap();
    timeout.join().unwrap();
    let events = sink.drain_events();
    assert_eq!(
        events
            .iter()
            .map(|event| (event.phase, event.terminal))
            .collect::<Vec<_>>(),
        [(0, false), (1, true)],
        "deadline terminal overtook paused worker: {terminal_overtook_worker}"
    );
    assert!(matches!(events[1].result, Err(ImageFailure::Timeout)));
    sink.cancel();
    sink.publish_next(scope, key, &phase, true, None, Ok(ImageUpdate::Finished));
    assert!(sink.drain_events().is_empty());
    assert_eq!(
        phase.load(Ordering::Relaxed),
        2,
        "cancelled streams allocate no further phases"
    );
}

#[tokio::test]
async fn performance_cancelled_sink_releases_queued_pixel_admission_and_rejects_late_events() {
    let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(2));
    let root = ImageSink::new();
    let request = root.for_request(5);
    let producer = request.with_admission(std::sync::Arc::new(
        permits.clone().acquire_owned().await.unwrap(),
    ));
    let scope = ImageScope {
        reader: 1,
        generation: 1,
    };
    producer.publish(
        scope,
        ImageKey::new("test"),
        0,
        false,
        None,
        Ok(ImageUpdate::Finished),
    );
    drop(producer);
    assert_eq!(
        permits.available_permits(),
        1,
        "queued bytes retain their decode admission"
    );
    request.cancel();
    assert_eq!(permits.available_permits(), 2);
    request.cancelled().await;
    request.publish(
        scope,
        ImageKey::new("test"),
        1,
        true,
        None,
        Ok(ImageUpdate::Finished),
    );
    assert!(root.drain_events().is_empty());
}

use super::*;
#[test]
fn performance_resized_transparent_edge_keeps_straight_alpha_without_a_dark_fringe() {
    let input = image::RgbaImage::from_raw(2, 1, vec![255, 255, 255, 255, 0, 0, 0, 0]).unwrap();
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(input)
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    let decoded = decode_for_renderer(
        encoded.into_inner(),
        &ImageLimits::default(),
        ImageRenderOptions::default().with_display_width(1.0),
        &|| Ok(()),
        &mut |_| panic!("static raster"),
    )
    .unwrap()
    .unwrap();
    assert!(!decoded.source_svg);
    let pixels = decoded.pixels.unwrap();
    assert_eq!((pixels.width, pixels.height), (1, 1));
    assert!(pixels.bgra[..3].iter().all(|channel| *channel >= 254));
    assert!((127..=128).contains(&pixels.bgra[3]));
}
#[test]
fn performance_static_renderer_preserves_gpui_exif_orientation() {
    let image = image::RgbImage::from_fn(6, 3, |x, y| {
        image::Rgb([(x * 40) as u8, (y * 80) as u8, 30])
    });
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 100)
        .encode_image(&image::DynamicImage::ImageRgb8(image))
        .unwrap();
    let raw = image::load_from_memory(&jpeg).unwrap().into_rgba8();
    for orientation in [3u8, 6] {
        let mut exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0".to_vec();
        exif.extend_from_slice(&[0x12, 1, 3, 0, 1, 0, 0, 0, orientation, 0, 0, 0, 0, 0, 0, 0]);
        let mut bytes = vec![0xff, 0xd8, 0xff, 0xe1];
        bytes.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
        bytes.extend_from_slice(&exif);
        bytes.extend_from_slice(&jpeg[2..]);
        let decoded = decode_for_renderer(
            bytes,
            &ImageLimits::default(),
            ImageRenderOptions::default(),
            &|| Ok(()),
            &mut |_| panic!("static JPEG"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            (decoded.width, decoded.height),
            (6, 3),
            "retain the reader's original logical header dimensions"
        );
        let mut expected = if orientation == 3 {
            image::imageops::rotate180(&raw)
        } else {
            image::imageops::rotate90(&raw)
        };
        for pixel in expected.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        let actual = decoded.pixels.unwrap();
        assert_eq!((actual.width, actual.height), expected.dimensions());
        assert_eq!(actual.bgra, expected.into_raw());
    }
}

#[test]
fn performance_animated_webp_keeps_both_original_frames() {
    fn chunk(out: &mut Vec<u8>, name: &[u8; 4], body: &[u8]) {
        out.extend_from_slice(name);
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
        if !body.len().is_multiple_of(2) {
            out.push(0);
        }
    }
    let mut chunks = Vec::new();
    chunk(&mut chunks, b"VP8X", &[2, 0, 0, 0, 5, 0, 0, 2, 0, 0]);
    chunk(&mut chunks, b"ANIM", &[0, 0, 0, 0, 0, 0]);
    for color in [[255u8, 0, 0, 255], [0, 255, 0, 255]] {
        let mut frame = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut frame)
            .encode(&color.repeat(18), 6, 3, image::ExtendedColorType::Rgba8)
            .unwrap();
        let mut anmf = vec![0, 0, 0, 0, 0, 0, 5, 0, 0, 2, 0, 0, 100, 0, 0, 2];
        anmf.extend_from_slice(&frame[12..]);
        chunk(&mut chunks, b"ANMF", &anmf);
    }
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&((chunks.len() + 4) as u32).to_le_bytes());
    bytes.extend_from_slice(b"WEBP");
    bytes.extend_from_slice(&chunks);
    let decoded = decode_for_renderer(
        bytes,
        &ImageLimits::default(),
        ImageRenderOptions::default(),
        &|| Ok(()),
        &mut |_| panic!("WebP is a looping raster"),
    )
    .unwrap()
    .unwrap();
    assert_eq!(decoded.raster_frames.len(), 2);
    assert_eq!(decoded.raster_frames[0].delay, (100, 1));
    assert_eq!(
        &decoded.raster_frames[0].pixels.bgra[..4],
        &[0, 0, 255, 255]
    );
    assert_eq!(
        &decoded.raster_frames[1].pixels.bgra[..4],
        &[0, 255, 0, 255]
    );
    assert_eq!(decoded.decoded_bytes(), 144);
}

use std::io::Cursor;
fn png(width: u32, height: u32) -> Vec<u8> {
    let rgba = image::RgbaImage::from_pixel(width, height, image::Rgba([10, 30, 90, 128]));
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}
#[test]
fn performance_static_pixels_preserve_straight_alpha_intrinsic_and_dpi() {
    let decoded = decode_for_renderer(
        png(120, 60),
        &ImageLimits::default(),
        ImageRenderOptions::for_device_scale(2.0).with_display_width(40.0),
        &|| Ok(()),
        &mut |_| panic!("static"),
    )
    .unwrap()
    .unwrap();
    assert_eq!((decoded.width, decoded.height), (120, 60));
    let pixels = decoded.pixels.unwrap();
    assert_eq!((pixels.width, pixels.height), (80, 40));
    assert_eq!(&pixels.bgra[..4], &[90, 30, 10, 128]);
    assert!(decoded.bytes.is_empty());
    let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="10" height="10" fill="#ff0000" fill-opacity=".5"/></svg>"##;
    let decoded = decode_for_renderer(
        svg.to_vec(),
        &ImageLimits::default(),
        ImageRenderOptions::for_device_scale(2.0).with_display_width(1.0),
        &|| Ok(()),
        &mut |_| panic!("static SVG"),
    )
    .unwrap()
    .unwrap();
    assert_eq!((decoded.width, decoded.height), (20, 10));
    let pixels = decoded.pixels.unwrap();
    assert_eq!(
        (pixels.width, pixels.height),
        (80, 40),
        "SVG supersampling ignores raster display reduction"
    );
    assert_eq!(&pixels.bgra[..4], &[0, 0, 255, 128]);
    assert_eq!(&pixels.bgra[(60 * 4)..(61 * 4)], &[0, 0, 0, 0]);
}
#[test]
fn performance_gif_keeps_frames_delays_and_charges_all_pixels() {
    let mut bytes = Vec::new();
    let frames = [image::Rgba([255, 0, 0, 255]), image::Rgba([0, 255, 0, 128])]
        .into_iter()
        .map(|color| {
            image::Frame::from_parts(
                image::RgbaImage::from_pixel(6, 3, color),
                0,
                0,
                image::Delay::from_numer_denom_ms(100, 1),
            )
        });
    image::codecs::gif::GifEncoder::new(&mut bytes)
        .encode_frames(frames)
        .unwrap();
    let decoded = decode_for_renderer(
        bytes.clone(),
        &ImageLimits::default(),
        ImageRenderOptions::default(),
        &|| Ok(()),
        &mut |_| panic!("GIF uses looping renderer frames"),
    )
    .unwrap()
    .unwrap();
    assert_eq!(decoded.raster_frames.len(), 2);
    assert_eq!(decoded.raster_frames[0].delay, (100, 1));
    assert_eq!(
        &decoded.raster_frames[0].pixels.bgra[..4],
        &[0, 0, 255, 255]
    );
    assert_eq!(decoded.decoded_bytes(), 6 * 3 * 4 * 2);
    assert!(decoded.bytes.is_empty());
    let limits = ImageLimits {
        max_decoded_bytes: 6 * 3 * 4,
        ..ImageLimits::default()
    };
    assert!(
        decode_for_renderer(
            bytes,
            &limits,
            ImageRenderOptions::default(),
            &|| Ok(()),
            &mut |_| Ok(())
        )
        .is_err()
    );
}
#[test]
fn performance_svg_publishes_first_before_later_frames_and_cancels_between_batches() {
    use std::cell::Cell;
    let stopped = Cell::new(false);
    let first = Cell::new(false);
    let result = decode_for_renderer(
        include_bytes!("fixtures/entry-stats.svg").to_vec(),
        &ImageLimits::default(),
        ImageRenderOptions::for_device_scale(2.0),
        &|| {
            if stopped.get() {
                Err(ImageFailure::Cancelled)
            } else {
                Ok(())
            }
        },
        &mut |update| {
            match update {
                RenderUpdate::First(decoded) => {
                    let animation = decoded.animation.unwrap();
                    assert_eq!(animation.frames.len(), 1);
                    assert_eq!(animation.frames[0].at_ms, 0);
                    assert_eq!(animation.frames[0].pixels.width, 480);
                    assert_eq!(animation.final_pixels.width, 960);
                    first.set(true);
                    stopped.set(true);
                }
                RenderUpdate::Frames(_) => panic!("cancellation stops subsequent batches"),
            }
            Ok(())
        },
    );
    assert!(first.get());
    assert_eq!(result, Err(ImageFailure::Cancelled));
}
