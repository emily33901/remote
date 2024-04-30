use egui::Color32;

pub(crate) fn interpolate_color(color1: Color32, color2: Color32, ratio: f32) -> Color32 {
    let (h1, s1, v1) = rgb_to_hsv(color1);
    let (h2, s2, v2) = rgb_to_hsv(color2);

    let h = interpolate(h1, h2, ratio);
    let s = interpolate(s1, s2, ratio);
    let v = interpolate(v1, v2, ratio);

    hsv_to_rgb(h, s, v)
}

fn rgb_to_hsv(color: Color32) -> (f32, f32, f32) {
    let r = color.r() as f32 / 255.0;
    let g = color.g() as f32 / 255.0;
    let b = color.b() as f32 / 255.0;

    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    let h = if delta == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / delta) % 6.0)
    } else if max == g {
        60.0 * (((b - r) / delta) + 2.0)
    } else {
        60.0 * (((r - g) / delta) + 4.0)
    };

    let s = if max == 0.0 { 0.0 } else { delta / max };

    let v = max;

    (h, s, v)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Color32 {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;

    let (r, g, b) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };

    let r = ((r + m) * 255.0).round() as u8;
    let g = ((g + m) * 255.0).round() as u8;
    let b = ((b + m) * 255.0).round() as u8;

    Color32::from_rgb(r, g, b)
}

fn interpolate(start: f32, end: f32, ratio: f32) -> f32 {
    start + (end - start) * ratio
}
