//! Non-generic boundary keeps SIMD monomorphizations in this optimized package.
//! Callers enforce source/output budgets and own all cancellation/admission.
use fast_image_resize::{
    PixelType, ResizeOptions, Resizer,
    images::{Image, TypedImage},
    pixels::U16x4,
};

#[derive(Debug)]
pub struct InvalidImage;

pub fn opaque_rgba(
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    target_width: u32,
    target_height: u32,
) -> Result<Vec<u8>, InvalidImage> {
    if target_width == 0 || target_height == 0 || target_width > width || target_height > height {
        return Err(InvalidImage);
    }
    let source =
        Image::from_vec_u8(width, height, pixels, PixelType::U8x4).map_err(|_| InvalidImage)?;
    let mut output = Image::new(target_width, target_height, PixelType::U8x4);
    Resizer::new()
        .resize(&source, &mut output, &ResizeOptions::new().use_alpha(false))
        .map_err(|_| InvalidImage)?;
    Ok(output.into_vec())
}

pub fn premultiplied_rgba16(
    width: u32,
    height: u32,
    pixels: Vec<[u16; 4]>,
    target_width: u32,
    target_height: u32,
) -> Result<Vec<[u16; 4]>, InvalidImage> {
    if target_width == 0 || target_height == 0 || target_width > width || target_height > height {
        return Err(InvalidImage);
    }
    let pixels = pixels.into_iter().map(U16x4::new).collect();
    let source = TypedImage::from_pixels(width, height, pixels).map_err(|_| InvalidImage)?;
    let mut output = TypedImage::<U16x4>::new(target_width, target_height);
    Resizer::new()
        .resize_typed(&source, &mut output, &ResizeOptions::new().use_alpha(false))
        .map_err(|_| InvalidImage)?;
    Ok(output.pixels().iter().map(|pixel| pixel.0).collect())
}
