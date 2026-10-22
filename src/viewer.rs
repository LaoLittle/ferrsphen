use atri_qq::media::MediaReference;

#[derive(Clone)]
pub struct ImageItem {
    pub key: String,
    pub reference: MediaReference,
}
pub struct Viewer {
    pub images: Vec<ImageItem>,
    pub index: usize,
    pub zoom: f32,
    pub pan: (f32, f32),
    pub dragging: Option<(f32, f32)>,
    pub error: Option<String>,
}
impl Viewer {
    pub fn new(images: Vec<ImageItem>, key: &str) -> Option<Self> {
        let index = images.iter().position(|image| image.key == key)?;
        Some(Self {
            images,
            index,
            zoom: 1.,
            pan: (0., 0.),
            dragging: None,
            error: None,
        })
    }
    pub fn current(&self) -> &ImageItem {
        &self.images[self.index]
    }
    pub fn navigate(&mut self, direction: isize) -> bool {
        let index = self.index.saturating_add_signed(direction);
        if index >= self.images.len() || index == self.index {
            return false;
        }
        self.index = index;
        self.reset();
        self.error = None;
        true
    }
    pub fn reset(&mut self) {
        self.zoom = 1.;
        self.pan = (0., 0.);
        self.dragging = None;
    }
    pub fn zoom_by(&mut self, factor: f32) {
        if !factor.is_finite() || factor <= 0. {
            return;
        }
        let next = (self.zoom * factor).clamp(0.1, 8.);
        self.pan.0 *= next / self.zoom;
        self.pan.1 *= next / self.zoom;
        self.zoom = next;
    }
}
pub fn forward_preview(reference: &atri_qq::messages::ForwardReference) -> Vec<String> {
    reference
        .light_app_json
        .as_ref()
        .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
        .and_then(|root| {
            root.pointer("/meta/detail/news")
                .and_then(serde_json::Value::as_array)
                .cloned()
        })
        .unwrap_or_default()
        .iter()
        .filter_map(|item| {
            item.get("text")
                .and_then(serde_json::Value::as_str)
                .or_else(|| item.as_str())
        })
        .take(3)
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use atri_qq::media::{MediaConversation, MediaKind};
    #[test]
    fn navigation_resets_transform_and_stops_at_boundaries() {
        let item = |key: &str| ImageItem {
            key: key.into(),
            reference: MediaReference::new(MediaKind::Image, MediaConversation::Group { uin: 1 }),
        };
        assert!(Viewer::new(vec![], "a").is_none());
        let mut viewer = Viewer::new(vec![item("a"), item("b")], "a").unwrap();
        assert!(!viewer.navigate(-1));
        viewer.zoom_by(2.);
        viewer.pan = (20., 10.);
        assert!(viewer.navigate(1));
        assert_eq!(viewer.current().key, "b");
        assert_eq!((viewer.zoom, viewer.pan), (1., (0., 0.)));
        assert!(!viewer.navigate(1));
        viewer.zoom_by(f32::INFINITY);
        assert_eq!(viewer.zoom, 1.);
        viewer.zoom_by(100.);
        assert_eq!(viewer.zoom, 8.);
    }
}
