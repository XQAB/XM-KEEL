//! A page's pictures, read into pixels and scaled to the room a page has
//! for them. The recovery carries no decoder, so PNG and JPEG come from
//! crates; a picture that will not read is left to the reader, which says
//! where it was instead.

use std::io::Cursor;

use crate::canvas::Canvas;

/// A picture as it was decoded: straight-alpha RGBA8, top row first.
pub struct Picture {
    pub w: u32,
    pub h: u32,
    pub pixels: Vec<u8>,
}

/// The picture `data` holds, if it is a PNG or a JPEG the decoders read.
pub fn read(data: &[u8]) -> Option<Picture> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        png(data)
    } else if data.starts_with(&[0xff, 0xd8]) {
        jpeg(data)
    } else {
        None
    }
}

fn png(data: &[u8]) -> Option<Picture> {
    let mut decoder = png::Decoder::new(Cursor::new(data));
    // Palettes become colours and 16-bit channels become 8-bit ones, so
    // every picture arrives in one of four shapes this knows.
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let src = &buf[..info.buffer_size()];
    let pixels = match info.color_type {
        png::ColorType::Rgba => src.to_vec(),
        png::ColorType::Rgb => four(src, 3, |p| [p[0], p[1], p[2], 255]),
        png::ColorType::Grayscale => src.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => four(src, 2, |p| [p[0], p[0], p[0], p[1]]),
        png::ColorType::Indexed => return None,
    };
    Some(Picture { w: info.width, h: info.height, pixels })
}

/// `src` read `n` bytes at a time, each group turned into one RGBA pixel:
/// the shapes a PNG and a JPEG come in are all a fixed number of channels
/// a pixel, and only the last of them is the alpha a picture keeps.
fn four(src: &[u8], n: usize, of: impl Fn(&[u8]) -> [u8; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len() / n * 4);
    let mut at = 0;
    while at + n <= src.len() {
        out.extend_from_slice(&of(&src[at..at + n]));
        at += n;
    }
    out
}

fn jpeg(data: &[u8]) -> Option<Picture> {
    let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(data));
    let rgb = decoder.decode().ok()?;
    let info = decoder.info()?;
    let pixels = four(&rgb, 3, |p| [p[0], p[1], p[2], 255]);
    Some(Picture { w: info.width as u32, h: info.height as u32, pixels })
}

impl Picture {
    /// This picture drawn `w` × `h`, each pixel the blend of the four it
    /// falls between: a page shows a picture smaller than it was, and a
    /// smaller picture shows as itself.
    pub fn canvas(&self, w: u32, h: u32) -> Canvas {
        let mut out = Canvas::new(w.max(1), h.max(1));
        let (sw, sh) = (self.w, self.h);
        if sw == 0 || sh == 0 || w == 0 || h == 0 {
            return out;
        }
        let at = |x: u32, y: u32| {
            let i = ((y.min(sh - 1) * sw + x.min(sw - 1)) * 4) as usize;
            [self.pixels[i] as f32, self.pixels[i + 1] as f32, self.pixels[i + 2] as f32, self.pixels[i + 3] as f32]
        };
        for dy in 0..h {
            let fy = ((dy as f32 + 0.5) * sh as f32 / h as f32 - 0.5).clamp(0.0, sh as f32 - 1.0);
            let y0 = fy.floor() as u32;
            let y1 = (y0 + 1).min(sh - 1);
            let wy = fy - y0 as f32;
            for dx in 0..w {
                let fx = ((dx as f32 + 0.5) * sw as f32 / w as f32 - 0.5).clamp(0.0, sw as f32 - 1.0);
                let x0 = fx.floor() as u32;
                let x1 = (x0 + 1).min(sw - 1);
                let wx = fx - x0 as f32;
                let corners = [(at(x0, y0), (1.0 - wx) * (1.0 - wy)), (at(x1, y0), wx * (1.0 - wy)), (at(x0, y1), (1.0 - wx) * wy), (at(x1, y1), wx * wy)];
                // Colour is weighted by how much of each pixel it covers and
                // how opaque that pixel was; the alpha is the cover itself.
                let (mut r, mut g, mut b, mut a) = (0.0, 0.0, 0.0, 0.0);
                for (px, weight) in corners {
                    let cover = weight * px[3] / 255.0;
                    r += px[0] * cover;
                    g += px[1] * cover;
                    b += px[2] * cover;
                    a += cover;
                }
                let i = ((dy * w + dx) * 4) as usize;
                if a > 0.0 {
                    out.data[i] = (r / a).round().clamp(0.0, 255.0) as u8;
                    out.data[i + 1] = (g / a).round().clamp(0.0, 255.0) as u8;
                    out.data[i + 2] = (b / a).round().clamp(0.0, 255.0) as u8;
                    out.data[i + 3] = (a * 255.0).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        out
    }
}

/// The picture `data` holds laid on `c` at (`x`, `y`) in the room `room`
/// pixels wide a page has for it, and the row under it; or `None` when it
/// is not a picture this reads.
pub fn drawn(c: &mut Canvas, data: &[u8], x: f32, y: f32, room: f32) -> Option<f32> {
    let pic = read(data)?;
    let room = room.max(1.0);
    let w = (pic.w as f32).min(room).round().max(1.0) as u32;
    let h = (pic.h as f32 * w as f32 / pic.w.max(1) as f32).round().max(1.0) as u32;
    let scaled = pic.canvas(w, h);
    c.blit(&scaled, x.round() as i32, y.round() as i32);
    Some(y + h as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_of(w: u32, h: u32, pixels: &[[u8; 4]]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        let flat: Vec<u8> = pixels.iter().flat_map(|p| *p).collect();
        writer.write_image_data(&flat).unwrap();
        writer.finish().unwrap();
        out
    }

    #[test]
    fn a_png_reads_as_the_pixels_it_holds() {
        let pixels = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 255, 0]];
        let picture = read(&png_of(2, 2, &pixels)).expect("a PNG reads");
        assert_eq!((picture.w, picture.h), (2, 2));
        assert_eq!(picture.pixels, pixels.iter().flat_map(|p| *p).collect::<Vec<u8>>());
    }

    #[test]
    fn what_is_not_a_picture_or_a_jpeg_is_not_read_as_one() {
        assert!(read(b"<html>a page whose pictures did not come</html>").is_none());
    }

    #[test]
    fn a_picture_narrowed_is_the_blend_of_what_each_pixel_covers() {
        let pixels = [[255, 0, 0, 255], [0, 0, 255, 255], [255, 0, 0, 255], [0, 0, 255, 255]];
        let picture = read(&png_of(2, 2, &pixels)).unwrap();
        let one = picture.canvas(1, 1);
        // Half red and half blue, both wholly opaque: the average of the two.
        assert_eq!(&one.data[0..4], &[128, 0, 128, 255]);
    }

    #[test]
    fn a_picture_lays_itself_on_a_page_and_says_where_it_ends() {
        let pixels = [[255, 255, 255, 255]; 200];
        let png = png_of(20, 10, &pixels);
        let mut page = Canvas::new(40, 40);
        // Twenty across, narrowed to the eight the page has room for, is
        // four tall: laid at row four it ends at row eight.
        let bottom = drawn(&mut page, &png, 4.0, 4.0, 8.0).expect("a PNG draws");
        assert_eq!(bottom, 8.0, "twenty across narrowed to eight is four tall, and it starts at four");
        let at = ((4 * 40 + 4) * 4) as usize;
        assert_eq!(&page.data[at..at + 4], &[255, 255, 255, 255], "the pixel where the picture was laid is the picture's");
    }
}
