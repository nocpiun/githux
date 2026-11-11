use super::*;
use base64::Engine;
use image::GenericImageView;

#[test]
fn quality_displacement_scale_is_applied_once_in_both_directions() {
    for (scale, inside, outside) in [(4, 22, 10), (-4, 30, 42)] {
        let svg = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="16"><defs><filter id="d" x="0" y="0" width="32" height="16" filterUnits="userSpaceOnUse" color-interpolation-filters="sRGB"><feFlood flood-color="rgb(255,128,128)" result="map"/><feDisplacementMap in="SourceGraphic" in2="map" scale="{scale}" xChannelSelector="R" yChannelSelector="G"/></filter></defs><rect x="12" y="4" width="4" height="8" fill="#f00" filter="url(#d)"/></svg>"##
        );
        let decoded = decode_image(svg.into_bytes(), &ImageLimits::default()).unwrap();
        let pixels = image::load_from_memory(&decoded.bytes).unwrap().to_rgba8();
        assert_eq!(
            pixels.get_pixel(inside, 12).0,
            [255, 0, 0, 255],
            "declared displacement {scale}"
        );
        assert_eq!(
            pixels.get_pixel(outside, 12).0[3],
            0,
            "scale must not be squared"
        );
    }
}

#[test]
fn quality_fractional_filter_grids_clip_transparent_edges_without_panicking() {
    for (x, y, width, height) in [
        (0.1, 0.1, 97.5, 62.5),
        (7.3, 11.3, 97.5, 62.5),
        (3.375, 5.125, 99.999, 61.25),
    ] {
        let svg = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="90"><defs><filter id="hand"><feTurbulence baseFrequency=".05" numOctaves="2" result="noise"/><feDisplacementMap in="SourceGraphic" in2="noise" scale="5" xChannelSelector="R" yChannelSelector="G"/></filter></defs><rect x="{x}" y="{y}" width="{width}" height="{height}" fill="#f00" filter="url(#hand)"/></svg>"##
        );
        for density in [1.0, 1.5, 2.0] {
            let decoded = decode_image_with_options_checked(
                svg.as_bytes().to_vec(),
                &ImageLimits::default(),
                ImageRenderOptions::for_device_scale(density),
                &|| Ok(()),
            )
            .unwrap();
            let pixels = image::load_from_memory(&decoded.bytes).unwrap().to_rgba8();
            let center = pixels.get_pixel(pixels.width() / 2, pixels.height() / 2).0;
            assert_eq!(
                center,
                [255, 0, 0, 255],
                "fractional clipped filter must retain the center at density {density}"
            );
        }
    }
}

#[test]
fn quality_displacement_accepts_canvas_clipped_source_and_full_map_grids() {
    // The filter exceeds resvg's four-canvas layer clip but remains well inside
    // the decoder budget. SourceGraphic is consequently smaller than the map;
    // the old equal-size assertion panicked even at integral display scales.
    let svg=br##"<svg xmlns="http://www.w3.org/2000/svg" width="12" height="9"><defs><filter id="hand" x="-1000%" y="-1000%" width="2100%" height="2100%"><feTurbulence baseFrequency=".05" numOctaves="2" result="noise"/><feDisplacementMap in="SourceGraphic" in2="noise" scale="5" xChannelSelector="R" yChannelSelector="G"/></filter></defs><rect x="1.1" y="1.3" width="9.75" height="6.25" fill="#f00" filter="url(#hand)"/></svg>"##;
    for density in [1.0, 1.5, 2.0] {
        let decoded = decode_image_with_options_checked(
            svg.to_vec(),
            &ImageLimits::default(),
            ImageRenderOptions::for_device_scale(density),
            &|| Ok(()),
        )
        .unwrap();
        let pixels = image::load_from_memory(&decoded.bytes).unwrap().to_rgba8();
        assert_eq!(
            pixels.get_pixel(pixels.width() / 2, pixels.height() / 2).0,
            [255, 0, 0, 255]
        );
    }
}

#[test]
fn quality_embedded_woff_alias_renders_the_supplied_glyphs() {
    let data = base64::engine::general_purpose::STANDARD
        .encode(include_bytes!("fixtures/embedded-font.woff"));
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="150" height="32"><style>@font-face{{font-family:DiagramFace;src:url(data:application/font-woff;base64,{data})}}</style><text x="2" y="24" font-size="20" font-family="DiagramFace">IWIWAB0123</text></svg>"#
    );
    let control=br#"<svg xmlns="http://www.w3.org/2000/svg" width="150" height="32"><text x="2" y="24" font-size="20" font-family="Google Sans Code">IWIWAB0123</text></svg>"#;
    let actual = decode_image(svg.into_bytes(), &ImageLimits::default()).unwrap();
    let expected = decode_image(control.to_vec(), &ImageLimits::default()).unwrap();
    assert_eq!(
        actual.bytes, expected.bytes,
        "embedded face aliases must select its glyphs instead of falling back to the UI face"
    );
}

#[test]
fn quality_woff_expanded_size_is_bounded_before_table_allocation() {
    let mut font = include_bytes!("fixtures/embedded-font.woff").to_vec();
    font[16..20].copy_from_slice(&(512u32 * 1024 * 1024).to_be_bytes());
    let data = base64::engine::general_purpose::STANDARD.encode(font);
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><style>@font-face{{font-family:Face;src:url(data:application/font-woff;base64,{data})}}</style><text y="8" font-family="Face">A</text></svg>"#
    );
    assert!(matches!(
        decode_image(svg.into_bytes(), &ImageLimits::default()),
        Err(ImageFailure::TooLarge { .. })
    ));
}

#[test]
fn quality_unbounded_font_collection_face_counts_are_not_walked() {
    let mut disguised = include_bytes!("fixtures/embedded-font.woff").to_vec();
    disguised[4..8].copy_from_slice(b"ttcf");
    for font in [b"ttcf\0\x01\0\0\xff\xff\xff\xff".to_vec(), disguised] {
        let data = base64::engine::general_purpose::STANDARD.encode(font);
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><style>@font-face{{font-family:Face;src:url(data:font/ttf;base64,{data})}}</style><text y="8" font-family="Face">A</text></svg>"#
        );
        assert!(matches!(
            decode_image(svg.into_bytes(), &ImageLimits::default()),
            Err(ImageFailure::Unsupported)
        ));
    }
}

#[test]
fn quality_sfnt_name_records_have_a_work_limit_independent_of_font_bytes() {
    // A compact directory can describe many overlapping name strings. Bound
    // that metadata before fontdb builds owned family names.
    let count = 257usize;
    let mut font = vec![0u8; 28 + 6 + count * 12];
    font[..4].copy_from_slice(b"OTTO");
    font[4..6].copy_from_slice(&1u16.to_be_bytes());
    font[12..16].copy_from_slice(b"name");
    font[20..24].copy_from_slice(&28u32.to_be_bytes());
    font[24..28].copy_from_slice(&((6 + count * 12) as u32).to_be_bytes());
    font[30..32].copy_from_slice(&(count as u16).to_be_bytes());
    let data = base64::engine::general_purpose::STANDARD.encode(font);
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><style>@font-face{{font-family:Face;src:url(data:font/otf;base64,{data})}}</style><text y="8" font-family="Face">A</text></svg>"#
    );
    assert!(matches!(
        decode_image(svg.into_bytes(), &ImageLimits::default()),
        Err(ImageFailure::TooLarge { .. })
    ));
}

#[test]
fn quality_svg_device_density_preserves_animation_and_crisp_final_grid() {
    for scale in [1.0, 1.5, 2.0, 3.0, 4.0] {
        let limits = ImageLimits::default();
        let decoded = decode_image_with_options_checked(
            include_bytes!("fixtures/entry-stats.svg").to_vec(),
            &limits,
            ImageRenderOptions::for_device_scale(scale),
            &|| Ok(()),
        )
        .unwrap();
        assert_eq!((decoded.width, decoded.height), (240, 100));
        let animation = decoded
            .animation
            .as_ref()
            .expect("density must not silently disable all entry animation");
        assert!(animation.frames.len() >= 2);
        let first = &animation.frames[0].pixels;
        assert!(
            first.width >= (240.0 * scale) as u32 && first.height >= (100.0 * scale) as u32,
            "physical density {scale}: {}x{}",
            first.width,
            first.height
        );
        assert!(
            animation
                .frames
                .iter()
                .all(|frame| (frame.pixels.width, frame.pixels.height)
                    == (first.width, first.height)),
            "the entry grid stays stable"
        );
        assert!(
            animation.final_pixels.width >= first.width
                && animation.final_pixels.height >= first.height
        );
        let sum = animation.final_pixels.bgra.len()
            + animation
                .frames
                .iter()
                .map(|frame| frame.pixels.bgra.len())
                .sum::<usize>();
        assert_eq!(decoded.decoded_bytes(), sum);
        assert!(sum <= limits.max_decoded_bytes);
        let png = image::load_from_memory(&decoded.bytes).unwrap();
        assert_eq!(
            png.dimensions(),
            (animation.final_pixels.width, animation.final_pixels.height)
        );
    }
}

#[test]
fn quality_explicit_none_and_pixel_dashoffset_reach_their_final_values() {
    let svg=br#"<svg xmlns="http://www.w3.org/2000/svg" width="50" height="20"><style>rect{transform:translateX(-15px);animation:move .2s linear forwards}path{stroke-dashoffset:20;animation:draw .2s linear forwards}@keyframes move{from{transform:translateX(-15px)}to{transform:none}}@keyframes draw{from{stroke-dashoffset:20px}to{stroke-dashoffset:0px}}</style><rect x="20" width="5" height="6" fill="red"/><path d="M5 14H25" stroke="blue" stroke-width="4" stroke-dasharray="20"/></svg>"#;
    let decoded = decode_image(svg.to_vec(), &ImageLimits::default()).unwrap();
    let final_pixels = image::load_from_memory(&decoded.bytes).unwrap().to_rgba8();
    let sx = final_pixels.width() / decoded.width;
    assert_eq!(final_pixels.get_pixel(22 * sx, 3 * sx).0, [255, 0, 0, 255]);
    assert_eq!(final_pixels.get_pixel(6 * sx, 3 * sx).0[3], 0);
    assert_eq!(final_pixels.get_pixel(15 * sx, 14 * sx).0, [0, 0, 255, 255]);
    let first = image::load_from_memory(&decoded.animation.unwrap().frames[0].bytes)
        .unwrap()
        .to_rgba8();
    assert_eq!(first.get_pixel(6, 3).0, [255, 0, 0, 255]);
    assert_eq!(first.get_pixel(15, 14).0[3], 0);
}
