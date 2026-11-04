//! Zooming the editor's text: View ▸ Zoom In, Zoom Out and Actual Size. The factor is kept with the preferences.

use crate::XmlCraftApp;
use crate::i18n::{fmt, tl};

/// The smallest and the largest factor, and the step of one zoom command.
pub const MIN: f32 = 0.5;
pub const MAX: f32 = 3.0;
pub const STEP: f32 = 0.1;

/// A factor that can be used: rounded to a whole step of [`STEP`] and kept from [`MIN`] to [`MAX`]; what is not a number (a hand-edited
/// preferences file) is normal size.
pub fn sanitized(factor: f32) -> f32 {
    if factor.is_finite() { (factor.clamp(MIN, MAX) / STEP).round() * STEP } else { 1.0 }
}

impl XmlCraftApp {
    /// How much larger than normal the editor's text is.
    pub fn editor_zoom(&self) -> f32 {
        self.editor_zoom
    }

    /// Zoom by `steps` (negative zooms out), or back to normal size with `None`.
    pub(crate) fn zoom_editor(&mut self, steps: Option<i32>) {
        self.editor_zoom = match steps {
            Some(n) => sanitized(self.editor_zoom + STEP * n as f32),
            None => 1.0,
        };
        self.status = fmt(tl!("Zoom {percent}%"), &[("percent", &((self.editor_zoom * 100.0).round() as i32).to_string())]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_factor_is_kept_in_range_and_on_a_step() {
        assert_eq!(sanitized(1.0), 1.0);
        assert!((sanitized(1.04) - 1.0).abs() < 1e-6);
        assert!((sanitized(1.26) - 1.3).abs() < 1e-6);
        assert_eq!(sanitized(100.0), MAX);
        assert_eq!(sanitized(0.0), MIN);
        assert_eq!(sanitized(-5.0), MIN);
        assert_eq!(sanitized(f32::NAN), 1.0);
        assert_eq!(sanitized(f32::INFINITY), 1.0, "not a number to trust");
    }
}
