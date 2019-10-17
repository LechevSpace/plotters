use crate::drawing::backend::{BackendCoord, DrawingBackend, DrawingErrorKind};
use crate::style::{Color, RGBAColor};
use image::{ImageBuffer, ImageError, Rgb, RgbImage};

use std::path::Path;

pub type BorrowedImage<'a> = ImageBuffer<Rgb<u8>, &'a mut [u8]>;

/// Macro implementation for drawing pixels, since a generic implementation would have been
/// much more unwieldy.
macro_rules! draw_pixel {
    ($img:expr, $point:expr, $color:expr) => {{
        if $point.0 as u32 >= $img.width()
            || $point.0 < 0
            || $point.1 as u32 >= $img.height()
            || $point.1 < 0
        {
            return Ok(());
        }

        let alpha = $color.alpha();
        let rgb = $color.rgb();

        if alpha >= 1.0 {
            $img.put_pixel($point.0 as u32, $point.1 as u32, Rgb([rgb.0, rgb.1, rgb.2]));
        } else {
            let pixel = $img.get_pixel_mut($point.0 as u32, $point.1 as u32);
            let new_color = [rgb.0, rgb.1, rgb.2];

            pixel.0.iter_mut().zip(&new_color).for_each(|(old, new)| {
                *old = (f64::from(*old) * (1.0 - alpha) + f64::from(*new) * alpha).min(255.0) as u8;
            });
        }
        Ok(())
    }};
}

#[cfg(feature = "gif")]
mod gif_support {
    use super::*;
    use gif::{Encoder as GifEncoder, Frame as GifFrame, Repeat, SetParameter};
    use std::fs::File;

    pub(super) struct GifFile {
        encoder: GifEncoder<File>,
        height: u32,
        width: u32,
        delay: u32,
    }

    impl GifFile {
        pub(super) fn new<T: AsRef<Path>>(
            path: T,
            dim: (u32, u32),
            delay: u32,
        ) -> Result<Self, ImageError> {
            let mut encoder = GifEncoder::new(
                File::create(path.as_ref()).map_err(ImageError::IoError)?,
                dim.0 as u16,
                dim.1 as u16,
                &[],
            )?;

            encoder.set(Repeat::Infinite)?;

            Ok(Self {
                encoder,
                width: dim.0,
                height: dim.1,
                delay: (delay + 5) / 10,
            })
        }

        pub(super) fn flush_frame(&mut self, img: &mut RgbImage) -> Result<(), ImageError> {
            let mut new_img = RgbImage::new(self.width, self.height);
            std::mem::swap(&mut new_img, img);

            let mut frame = GifFrame::from_rgb_speed(
                self.width as u16,
                self.height as u16,
                &new_img.into_raw(),
                10,
            );

            frame.delay = self.delay as u16;

            self.encoder.write_frame(&frame)?;

            Ok(())
        }
    }
}

enum Target<'a> {
    File(&'a Path, RgbImage),
    Buffer(BorrowedImage<'a>),
    #[cfg(feature = "gif")]
    Gif(Box<gif_support::GifFile>, RgbImage),
}

/// The backend that drawing a bitmap
pub struct BitMapBackend<'a> {
    /// The path to the image
    target: Target<'a>,

    /// Flag indicates if the bitmap has been saved
    saved: bool,
}

impl<'a> BitMapBackend<'a> {
    /// Create a new bitmap backend
    pub fn new<T: AsRef<Path> + ?Sized>(path: &'a T, dimension: (u32, u32)) -> Self {
        Self {
            target: Target::File(path.as_ref(), RgbImage::new(dimension.0, dimension.1)),
            saved: false,
        }
    }

    /// Create a new bitmap backend that generate GIF animation
    ///
    /// When this is used, the bitmap backend acts similar to a realtime rendering backend.
    /// When the program finished drawing one frame, use `present` function to flush the frame
    /// into the GIF file.
    ///
    /// - `path`: The path to the GIF file to create
    /// - `dimension`: The size of the GIF image
    /// - `speed`: The amount of time for each frame to display
    #[cfg(feature = "gif")]
    pub fn gif<T: AsRef<Path>>(
        path: T,
        dimension: (u32, u32),
        frame_delay: u32,
    ) -> Result<Self, ImageError> {
        Ok(Self {
            target: Target::Gif(
                Box::new(gif_support::GifFile::new(path, dimension, frame_delay)?),
                RgbImage::new(dimension.0, dimension.1),
            ),
            saved: false,
        })
    }

    /// Create a new bitmap backend which only lives in-memory
    pub fn with_buffer(buf: &'a mut [u8], dimension: (u32, u32)) -> Self {
        Self {
            target: Target::Buffer(
                BorrowedImage::from_raw(dimension.0, dimension.1, buf)
                    .expect("Buffer size must match dimensions (w * h * 3)."),
            ),
            saved: false,
        }
    }
}

impl<'a> DrawingBackend for BitMapBackend<'a> {
    type ErrorType = ImageError;

    fn get_size(&self) -> (u32, u32) {
        match &self.target {
            Target::Buffer(img) => (img.width(), img.height()),
            Target::File(_, img) => (img.width(), img.height()),
            #[cfg(feature = "gif")]
            Target::Gif(_, img) => (img.width(), img.height()),
        }
    }

    fn ensure_prepared(&mut self) -> Result<(), DrawingErrorKind<ImageError>> {
        self.saved = false;
        Ok(())
    }

    fn present(&mut self) -> Result<(), DrawingErrorKind<ImageError>> {
        match &mut self.target {
            Target::File(path, img) => {
                img.save(&path)
                    .map_err(|x| DrawingErrorKind::DrawingError(ImageError::IoError(x)))?;
                self.saved = true;
                Ok(())
            }
            Target::Buffer(_) => Ok(()),

            #[cfg(feature = "gif")]
            Target::Gif(target, img) => {
                target
                    .flush_frame(img)
                    .map_err(DrawingErrorKind::DrawingError)?;
                self.saved = true;
                Ok(())
            }
        }
    }

    fn draw_pixel(
        &mut self,
        point: BackendCoord,
        color: &RGBAColor,
    ) -> Result<(), DrawingErrorKind<ImageError>> {
        match &mut self.target {
            Target::Buffer(img) => draw_pixel!(img, point, color),
            Target::File(_, img) => draw_pixel!(img, point, color),
            #[cfg(feature = "gif")]
            Target::Gif(_, img) => draw_pixel!(img, point, color),
        }
    }

    fn blit_bitmap<'b>(
        &mut self,
        pos: BackendCoord,
        src: &'b image::ImageBuffer<image::Rgb<u8>, &'b [u8]>,
    ) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        let (dw, dh) = self.get_size();
        let (sw, sh) = src.dimensions();

        let (x0, y0) = pos;
        let (x1, y1) = (x0 + sw as i32, y0 + sh as i32);

        let (x0, y0, x1, y1) = (x0.max(0), y0.max(0), x1.min(dw as i32), y1.min(dh as i32));

        if x0 == x1 || y0 == y1 {
            return Ok(());
        }

        let mut chunk_size = (x1 - x0) as usize;
        let mut num_chunks = (y1 - y0) as usize;
        let dst_gap = dw as usize - chunk_size;
        let src_gap = sw as usize - chunk_size;

        let dst_start = 3 * (y0 as usize * dw as usize + x0 as usize);

        let mut dst = match &mut self.target {
            Target::File(_, img) => &mut (**img)[dst_start..],
            Target::Buffer(img) => &mut (**img)[dst_start..],
            #[cfg(feature = "gif")]
            Target::Gif(_, img) => &mut (**img)[dst_start..],
        };

        let src_start = 3 * ((sh as usize + y0 as usize - y1 as usize) * sw as usize + (sw as usize + x0 as usize - x1 as usize) as usize);
        let mut src = &(**src)[src_start..];

        if src_gap == 0 && dst_gap == 0 {
            chunk_size *= num_chunks;
            num_chunks = 1;
        }
        for i in 0..num_chunks {
            dst[0..(chunk_size * 3)].copy_from_slice(&src[0..(chunk_size * 3)]);
            if i != num_chunks - 1 {
                dst = &mut dst[((chunk_size + dst_gap) * 3)..];
                src = &src[((chunk_size + src_gap) * 3)..];
            }
        }

        Ok(())
    }
}

impl Drop for BitMapBackend<'_> {
    fn drop(&mut self) {
        if !self.saved {
            self.present().expect("Unable to save the bitmap");
        }
    }
}

#[cfg(test)]
#[test]
fn test_bitmap_backend() {
    use crate::prelude::*;
    let mut buffer = vec![0; 10 * 10 * 3];

    {
        let back = BitMapBackend::with_buffer(&mut buffer, (10, 10));

        let area = back.into_drawing_area();
        area.fill(&WHITE).unwrap();
        area.draw(&Path::new(vec![(0, 0), (10, 10)], RED.filled()))
            .unwrap();
        area.present().unwrap();
    }

    for i in 0..10 {
        assert_eq!(buffer[i * 33], 255);
        assert_eq!(buffer[i * 33 + 1], 0);
        assert_eq!(buffer[i * 33 + 2], 0);
        buffer[i * 33] = 255;
        buffer[i * 33 + 1] = 255;
        buffer[i * 33 + 2] = 255;
    }

    assert!(buffer.into_iter().all(|x| x == 255));
}
