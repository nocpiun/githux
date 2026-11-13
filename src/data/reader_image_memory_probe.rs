use super::*;
#[cfg(windows)]
fn working_set() -> Option<(usize, usize)> {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        faults: u32,
        peak_working: usize,
        working: usize,
        peak_paged: usize,
        paged: usize,
        peak_nonpaged: usize,
        nonpaged: usize,
        pagefile: usize,
        peak_pagefile: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, size: u32) -> i32;
    }
    let mut counters = Counters {
        cb: std::mem::size_of::<Counters>() as u32,
        ..Default::default()
    };
    // -1 is Windows' current-process pseudo handle. The C-layout buffer is
    // writable for its exact declared size and lives throughout this call.
    let size = counters.cb;
    let ok = unsafe { K32GetProcessMemoryInfo(-1, &mut counters, size) };
    (ok != 0).then_some((counters.working, counters.peak_working))
}
#[cfg(not(windows))]
fn working_set() -> Option<(usize, usize)> {
    None
}

#[test]
#[ignore = "explicit isolated original-byte/DPI memory measurement"]
fn performance_nine_originals_memory_probe() {
    use crate::content::images::decode_for_renderer;
    let rgba = image::RgbaImage::from_fn(2559, 1520, |x, y| {
        image::Rgba([
            ((x / 13 + y / 11) % 256) as u8,
            ((x / 7) % 256) as u8,
            ((y / 5) % 256) as u8,
            255,
        ])
    });
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    let original = encoded.into_inner();
    let before = working_set();
    let mut retained = Vec::new();
    for _ in 0..9 {
        let decoded = decode_for_renderer(
            original.clone(),
            &ImageLimits::default(),
            ImageRenderOptions::for_device_scale(2.0).with_display_width(600.0),
            &|| Ok(()),
            &mut |_| panic!("static original"),
        )
        .unwrap()
        .unwrap();
        assert_eq!((decoded.width, decoded.height), (2559, 1520));
        assert_eq!(decoded.pixels.as_ref().unwrap().width, 1200);
        retained.push(decoded);
    }
    let retained_bytes = retained
        .iter()
        .map(DecodedImage::decoded_bytes)
        .sum::<usize>();
    assert!(retained_bytes < 64 * 1024 * 1024);
    eprintln!(
        "nine_originals_memory: original_encoded_each={} native_pixels_total={} retained_pixels={} before_working_and_peak={:?} after_working_and_peak={:?} logical_width=600 dpi=2",
        original.len(),
        9 * 2559 * 1520 * 4,
        retained_bytes,
        before,
        working_set()
    );
}
