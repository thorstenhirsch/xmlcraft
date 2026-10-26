//! Whether the window animates. Preferences ▸ Animation effects and the operating system's "reduce motion" request arrive with
//! the preferences milestone (see docs/gaps.md); until then the choice is egui's own animation time, which a test or a user
//! setting can set to zero.

/// True when hover washes, fades and eased scrolling should run.
pub fn animated(ctx: &egui::Context) -> bool {
    ctx.global_style().animation_time > 0.0
}

#[cfg(test)]
mod tests {
    use super::animated;

    #[test]
    fn zero_animation_time_means_no_animation() {
        let ctx = egui::Context::default();
        assert!(animated(&ctx), "egui animates by default");
        ctx.global_style_mut(|s| s.animation_time = 0.0);
        assert!(!animated(&ctx));
    }
}
