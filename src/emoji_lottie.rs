use anyhow::{Result, ensure};
use velato::{
    Composition, RenderSink,
    model::{ImageAsset, fixed},
};
use vello_cpu::{
    Pixmap, RenderContext, Resources,
    kurbo::{Affine, Shape},
    peniko::{BlendMode, Brush},
};

pub(crate) struct Animation {
    composition: Composition,
    pub delays: Vec<std::time::Duration>,
}
impl Animation {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let composition = Composition::from_slice(bytes)?;
        ensure!(
            (1..=4096).contains(&composition.width) && (1..=4096).contains(&composition.height),
            "invalid Lottie dimensions"
        );
        ensure!(
            composition.frame_rate.is_finite() && (1.0..=120.0).contains(&composition.frame_rate),
            "invalid Lottie frame rate"
        );
        let count = (composition.frames.end - composition.frames.start).ceil();
        ensure!(
            count.is_finite() && (1.0..=3600.0).contains(&count),
            "invalid Lottie duration"
        );
        ensure!(
            composition.images.is_empty(),
            "external Lottie images are unsupported"
        );
        let delays =
            vec![std::time::Duration::from_secs_f64(1.0 / composition.frame_rate); count as usize];
        Ok(Self {
            composition,
            delays,
        })
    }
    pub fn render(&self, frame: usize, edge: u16) -> image::RgbaImage {
        let mut sink = CpuSink(RenderContext::new(edge, edge));
        let scale = f64::from(edge) / self.composition.width.max(self.composition.height) as f64;
        let transform = Affine::translate((
            (f64::from(edge) - self.composition.width as f64 * scale) * 0.5,
            (f64::from(edge) - self.composition.height as f64 * scale) * 0.5,
        )) * Affine::scale(scale);
        velato::Renderer::new().append(
            &self.composition,
            self.composition.frames.start + frame.min(self.delays.len() - 1) as f64,
            transform,
            1.0,
            &mut sink,
        );
        sink.0.flush();
        let mut pixels = Pixmap::new(edge, edge);
        sink.0.render(&mut pixels, &mut Resources::new());
        let mut bytes = pixels.data_as_u8_slice().to_vec();
        // GPUI expects straight BGRA; the CPU renderer returns premultiplied RGBA.
        for pixel in bytes.as_chunks_mut::<4>().0 {
            let alpha = u32::from(pixel[3]);
            for component in &mut pixel[..3] {
                *component = (u32::from(*component) * 255 + alpha / 2)
                    .checked_div(alpha)
                    .unwrap_or(0)
                    .min(255) as u8;
            }
            pixel.swap(0, 2);
        }
        image::RgbaImage::from_raw(u32::from(edge), u32::from(edge), bytes)
            .expect("valid pixmap size")
    }
}
struct CpuSink(RenderContext);
impl RenderSink for CpuSink {
    fn push_layer(
        &mut self,
        blend: impl Into<BlendMode>,
        alpha: f32,
        transform: Affine,
        shape: &impl Shape,
    ) {
        self.0.set_transform(transform);
        self.0.push_layer(
            Some(&shape.to_path(0.1)),
            Some(blend.into()),
            Some(alpha),
            None,
            None,
        );
    }
    fn push_clip_layer(&mut self, transform: Affine, shape: &impl Shape) {
        self.0.set_transform(transform);
        self.0.push_clip_layer(&shape.to_path(0.1));
    }
    fn pop_layer(&mut self) {
        self.0.pop_layer();
    }
    fn draw(
        &mut self,
        stroke: Option<&fixed::Stroke>,
        transform: Affine,
        brush: &fixed::Brush,
        shape: &impl Shape,
    ) {
        self.0.set_transform(transform);
        self.0.set_paint_transform(Affine::IDENTITY);
        match brush {
            Brush::Solid(color) => self.0.set_paint(*color),
            Brush::Gradient(gradient) => self.0.set_paint(gradient.clone()),
            Brush::Image(_) => return,
        }
        let path = shape.to_path(0.1);
        if let Some(stroke) = stroke {
            self.0.set_stroke(stroke.clone());
            self.0.stroke_path(&path);
        } else {
            self.0.fill_path(&path);
        }
    }
    fn draw_image(&mut self, _: &ImageAsset, _: Affine, _: f64) {}
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn moving_circle() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "v":"5.7.4","fr":30,"ip":0,"op":30,"w":100,"h":100,
            "layers":[{"ty":4,"ind":1,"ip":0,"op":30,"st":0,
                "ks":{"o":{"a":0,"k":100},"r":{"a":0,"k":0},"a":{"a":0,"k":[0,0,0]},"s":{"a":0,"k":[100,100,100]},
                    "p":{"a":1,"k":[{"t":0,"s":[20,50,0],"e":[80,50,0],"o":{"x":0.33,"y":0.33},"i":{"x":0.67,"y":0.67}},{"t":30,"s":[80,50,0]}]}},
                "shapes":[{"ty":"el","p":{"a":0,"k":[0,0]},"s":{"a":0,"k":[24,24]}},
                    {"ty":"fl","c":{"a":0,"k":[1,0.2,0,1]},"o":{"a":0,"k":100},"r":1}]
            }]
        })).unwrap()
    }
    #[test]
    fn lottie_renders_translated_color_and_alpha_at_requested_resolution() {
        let animation = Animation::parse(&moving_circle()).unwrap();
        assert_eq!(animation.delays.len(), 30);
        let first = animation.render(0, 100);
        let later = animation.render(20, 100);
        assert_eq!(first.dimensions(), (100, 100));
        assert!(
            first.get_pixel(20, 50)[2] > 240,
            "red must be in the BGRA red channel"
        );
        assert_eq!(first.get_pixel(80, 50)[3], 0);
        assert_ne!(first.as_raw(), later.as_raw());
        assert_eq!(animation.render(20, 48).dimensions(), (48, 48));
    }
    #[test]
    fn lottie_rejects_invalid_timing_and_external_images() {
        let mut data: serde_json::Value = serde_json::from_slice(&moving_circle()).unwrap();
        data["fr"] = 0.into();
        assert!(Animation::parse(&serde_json::to_vec(&data).unwrap()).is_err());
        data["fr"] = 30.into();
        data["assets"] = serde_json::json!([{"id":"image","w":10,"h":10,"u":"https://example.invalid/","p":"image.png","e":0}]);
        assert!(Animation::parse(&serde_json::to_vec(&data).unwrap()).is_err());
    }
    #[tokio::test]
    #[ignore = "downloads official Noto assets"]
    async fn official_noto_lottie_assets_parse_and_render() {
        let data: serde_json::Value =
            serde_json::from_str(include_str!("../assets/noto-emoji-map.json")).unwrap();
        let client = reqwest::Client::new();
        for face in data["faces"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|face| face["animated"] == true)
        {
            let unicode = face["unicode"].as_str().unwrap();
            let bytes = client
                .get(format!(
                    "https://fonts.gstatic.com/s/e/notoemoji/latest/{unicode}/lottie.json"
                ))
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .bytes()
                .await
                .unwrap();
            let animation =
                Animation::parse(&bytes).unwrap_or_else(|error| panic!("{unicode}: {error}"));
            let frame = animation.delays.len() / 2;
            let rendered = animation.render(frame, 96);
            assert!(
                rendered.pixels().any(|pixel| pixel[3] > 128),
                "{unicode} rendered empty"
            );
            if let Ok(directory) = std::env::var("FERRSPHEN_EMOJI_CAPTURE_DIR") {
                let mut rgba = rendered;
                for pixel in rgba.pixels_mut() {
                    pixel.0.swap(0, 2);
                }
                rgba.save(std::path::Path::new(&directory).join(format!("{unicode}.png")))
                    .unwrap();
            }
        }
    }
}
