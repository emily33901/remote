use openh264::formats::YUVSource;

pub trait YUVSource2 {
    fn y_mut(&mut self) -> &mut [u8];
    fn u_mut(&mut self) -> &mut [u8];
    fn v_mut(&mut self) -> &mut [u8];
}

pub struct YUVBuffer2 {
    yuv: Vec<u8>,
    width: usize,
    height: usize,
}

impl YUVBuffer2 {
    /// Allocates a new YUV buffer with the given width and height.
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            yuv: vec![0u8; (3 * (width * height)) / 2],
            width,
            height,
        }
    }

    /// Allocates a new YUV buffer with the given width and height and data.
    ///
    /// Data `rgb` is assumed to be `[rgb rgb rgb ...]`, starting at `y = 0`, continuing downwards, in other words
    /// how you'd naively store an RGB image buffer.
    ///
    /// # Panics
    ///
    /// Will panic if `rgb` does not match the formats given.
    pub fn with_rgb(width: usize, height: usize, rgb: &[u8]) -> Self {
        let mut rval = Self {
            yuv: vec![0u8; (3 * (width * height)) / 2],
            width,
            height,
        };

        rval.read_rgb(rgb);
        rval
    }

    /// Reads an RGB buffer, converts it to YUV and stores it.
    ///
    /// Data `rgb` is assumed to be `[rgb rgb rgb ...]`, starting at `y = 0`, continuing downwards, in other words
    /// how you'd naively store an RGB image buffer.
    ///
    /// # Panics
    ///
    /// Will panic if `rgb` does not match the formats given.
    pub fn read_rgb(&mut self, rgb: &[u8]) {
        let width = self.width;
        let height = self.height;

        let u_base = width * height;
        let v_base = u_base + u_base / 4;
        let half_width = width / 2;

        assert_eq!(rgb.len(), width * height * 3);
        assert_eq!(width % 2, 0, "width needs to be multiple of 2");
        assert_eq!(height % 2, 0, "height needs to be a multiple of 2");

        // y is full size, u, v is quarter size
        let pixel = |x: usize, y: usize| -> (f32, f32, f32) {
            // two dim to single dim
            let base_pos = (x + y * width) * 3;
            (
                rgb[base_pos] as f32,
                rgb[base_pos + 1] as f32,
                rgb[base_pos + 2] as f32,
            )
        };

        let write_y = |yuv: &mut [u8], x: usize, y: usize, rgb: (f32, f32, f32)| {
            yuv[x + y * width] =
                (0.2578125 * rgb.0 + 0.50390625 * rgb.1 + 0.09765625 * rgb.2 + 16.0) as u8;
        };

        let write_u = |yuv: &mut [u8], x: usize, y: usize, rgb: (f32, f32, f32)| {
            yuv[u_base + x + y * half_width] =
                (-0.1484375 * rgb.0 + -0.2890625 * rgb.1 + 0.4375 * rgb.2 + 128.0) as u8;
        };

        let write_v = |yuv: &mut [u8], x: usize, y: usize, rgb: (f32, f32, f32)| {
            yuv[v_base + x + y * half_width] =
                (0.4375 * rgb.0 + -0.3671875 * rgb.1 + -0.0703125 * rgb.2 + 128.0) as u8;
        };

        for i in 0..width / 2 {
            for j in 0..height / 2 {
                let px = i * 2;
                let py = j * 2;
                let pix0x0 = pixel(px, py);
                let pix0x1 = pixel(px, py + 1);
                let pix1x0 = pixel(px + 1, py);
                let pix1x1 = pixel(px + 1, py + 1);
                let avg_pix = (
                    (pix0x0.0 as u32 + pix0x1.0 as u32 + pix1x0.0 as u32 + pix1x1.0 as u32) as f32
                        / 4.0,
                    (pix0x0.1 as u32 + pix0x1.1 as u32 + pix1x0.1 as u32 + pix1x1.1 as u32) as f32
                        / 4.0,
                    (pix0x0.2 as u32 + pix0x1.2 as u32 + pix1x0.2 as u32 + pix1x1.2 as u32) as f32
                        / 4.0,
                );
                write_y(&mut self.yuv[..], px, py, pix0x0);
                write_y(&mut self.yuv[..], px, py + 1, pix0x1);
                write_y(&mut self.yuv[..], px + 1, py, pix1x0);
                write_y(&mut self.yuv[..], px + 1, py + 1, pix1x1);
                write_u(&mut self.yuv[..], i, j, avg_pix);
                write_v(&mut self.yuv[..], i, j, avg_pix);
            }
        }
    }

    pub(crate) fn y_mut(&mut self) -> &mut [u8] {
        &mut self.yuv[0..self.width * self.height]
    }

    pub(crate) fn u_mut(&mut self) -> &mut [u8] {
        let base_u = self.width * self.height;
        &mut self.yuv[base_u..base_u + base_u / 4]
    }

    pub(crate) fn v_mut(&mut self) -> &mut [u8] {
        let base_u = self.width * self.height;
        let base_v = base_u + base_u / 4;
        &mut self.yuv[base_v..]
    }

    pub(crate) fn yuv_mut(&mut self) -> (&mut [u8], &mut [u8], &mut [u8]) {
        let (y, a) = self.yuv.split_at_mut(self.width * self.height);
        let base_u = self.width * self.height;
        let (u, v) = a.split_at_mut(base_u / 4);

        (y, u, v)
    }

    pub(crate) fn buffer_mut(&mut self) -> &mut [u8] {
        &mut self.yuv
    }
}

impl YUVSource for YUVBuffer2 {
    fn width(&self) -> i32 {
        self.width as i32
    }

    fn height(&self) -> i32 {
        self.height as i32
    }

    fn y(&self) -> &[u8] {
        &self.yuv[0..self.width * self.height]
    }

    fn u(&self) -> &[u8] {
        let base_u = self.width * self.height;
        &self.yuv[base_u..base_u + base_u / 4]
    }

    fn v(&self) -> &[u8] {
        let base_u = self.width * self.height;
        let base_v = base_u + base_u / 4;
        &self.yuv[base_v..]
    }

    fn y_stride(&self) -> i32 {
        self.width as i32
    }

    fn u_stride(&self) -> i32 {
        (self.width / 2) as i32
    }

    fn v_stride(&self) -> i32 {
        (self.width / 2) as i32
    }
}
