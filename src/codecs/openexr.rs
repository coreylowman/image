//! Decoding of OpenEXR (.exr) Images
//!
//! OpenEXR is an image format that is widely used, especially in VFX,
//! because it supports lossless and lossy compression for float data.
//!
//! # Related Links
//! * <https://www.openexr.com/documentation.html> - The OpenEXR reference.

// # ROADMAP
// - [] ImageDecoder
// - [] ImageEncoder
// - [] GenericImageView?
// - [] GenericImage?
// - [] Progress

// # MAYBE SOON
// - [] ImageDecoderExt::read_rect_with_progress
// - [] Layers -> Animation?


extern crate exr;
use exr::prelude::*;

use crate::{ImageDecoder, ImageEncoder, ImageResult, ColorType, ExtendedColorType, Progress, image, ImageError, ImageFormat};
use std::io::{Read, Write, Seek, BufRead, SeekFrom, Cursor};
use self::exr::image::read::layers::FirstValidLayerReader;
use std::cell::Cell;
use crate::error::{DecodingError, ImageFormatHint};
use self::exr::meta::header::Header;

/// An OpenEXR decoder
#[derive(Debug)]
pub struct ExrDecoder<R> {
    source: R,
    header: exr::meta::header::Header,
}



impl<R: BufRead + Seek> ExrDecoder<R> {

    /// Create a decoder. Consumes the first few bytes of the source to extract image dimensions.
    pub fn from_buffered(mut source: R) -> ImageResult<Self> {

        // read meta data, then go back to the start of the file
        let mut meta_data = {
            let start_position = source.stream_position()?;
            let meta: MetaData = exr::meta::MetaData::read_from_buffered(&mut source, true)?;
            source.seek(SeekFrom::Start(start_position))?;
            meta
        };

        let header: Header = meta_data.headers.into_iter()
            .filter(|header|{
                let has_rgb = ["R","G","B"].iter().all(
                    |required| header.channels.list.iter().find(|chan| chan.name.eq(required)).is_some() // TODO eq_lowercase only if exrs supports it
                );

                !header.deep && has_rgb
            })
            .next()
            .ok_or_else(|| ImageError::Decoding(DecodingError::new(
                ImageFormatHint::Exact(ImageFormat::Exr),
                "image does not contain non-deep rgb channels"
            )))?;

        Ok(Self { source, header })
    }
}


impl<'a, R: 'a + Read> ImageDecoder<'a> for ExrDecoder<R> {
    type Reader = Cursor<Vec<u8>>;

    fn dimensions(&self) -> (u32, u32) {
        let s = self.header.layer_size;
        (s.width() as u32, s.height() as u32)
    }

    // TODO fn original_color_type(&self) -> ExtendedColorType {}

    fn color_type(&self) -> ColorType {
        // TODO adapt to actual exr color type
        ColorType::Rgba32
    }

    /// Use `read_image` if possible,
    /// as this method creates a whole new buffer to contain the entire image.
    fn into_reader(self) -> ImageResult<Self::Reader> {
        Ok(Cursor::new(image::decoder_to_vec(self)?))
    }

    fn scanline_bytes(&self) -> u64 {
        // we cannot always read individual scan lines,
        // as the tiles or lines in the file could be in random order.
        // therefore we currently read all lines at once
        // Todo: respect exr.line_order
       self.total_bytes()
    }

    fn read_image(mut self, target: &mut [u8]) -> ImageResult<()> {
        self.read_image_with_progress(target, |_|{})
    }

    fn read_image_with_progress<F: Fn(Progress)>(self, target: &mut [u8], progress_callback: F) -> ImageResult<()> {
        // TODO reuse meta-data from earlier

        let result = exr::prelude::read()
            .no_deep_data().largest_resolution_level()
            .rgba_channels(
                // FIXME no alloc+copy, write into target slice directly!
                |size, _channels| (
                    size, vec![0.0_f32; size.area()*4]
                ),

                |&(size, buffer), position, (r,g,b,a_or_1): (f32,f32,f32,f32)| {
                    // TODO white point chromaticities + srgb/linear conversion?
                    let first_f32_index = position.flat_index_for_size(size);
                    buffer[first_f32_index*4 + (first_f32_index + 1)*4].copy_from_slice([r, g, b, a_or_1]);
                }
            )
            .first_valid_layer().all_attributes()
            .on_progress(|progress| progress_callback(Progress::new((progress*100.0) as u64, 100_u64)))
            .from_buffered(self.source)?;

        target.copy_from_slice(bytemuck::bytes_of(result.layer_data.channel_data.pixels.1.as_slice()));
        // TODO keep meta data?

        Ok(())
    }
}



fn write_image(out: impl Write + Seek, bytes: &[u8], width: u32, height: u32, color_type: ColorType) -> ImageResult<()> {
    let pixels: &[f32] = bytemuck::try_from_bytes(bytes).expect("byte buffer must be aligned to f32");

    match color_type {
        ColorType::Rgb32 => {
            exr::prelude::Image::from_channels(
                (width as usize, height as usize),
                SpecificChannels::rgb(|pixel: Vec2<usize>| {
                    let pixel_index = 3 * pixel.flat_index_for_size(Vec2(width as usize, height as usize));
                    (pixels[pixel_index], pixels[pixel_index+1], pixels[pixel_index+2])
                })
            ).write().to_buffered(out)?;
        }

        ColorType::Rgba32 => {
            exr::prelude::Image::from_channels(
                (width as usize, height as usize),
                SpecificChannels::rgba(|pixel: Vec2<usize>| {
                    let pixel_index = 4 * pixel.flat_index_for_size(Vec2(width as usize, height as usize));
                    (pixels[pixel_index], pixels[pixel_index+1], pixels[pixel_index+2], pixels[pixel_index+3])
                })
            ).write().to_buffered(out)?;
        }

        _ => todo!()
    }

    Ok(())
}

