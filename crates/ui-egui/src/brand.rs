//! RECut: the RECut wordmark, drawn in code (original work for RECut, no image asset).
//!
//! RECut is a modified version of FilmCraft. The ArtCraft brand licence (`docs/brand/LICENSE-brand.txt`)
//! asks modified versions to remove the ArtCraft marks, so the ArtCraft wordmark that FilmCraft draws here
//! is replaced by RECut's own: a red recording dot followed by "RECut". See `RECUT.md`.

/// The product name shown in the interface.
pub const APP_NAME: &str = "RECut";

/// Plain-text credit to the project RECut is based on (allowed by the brand licence).
pub const BASED_ON: &str = "Basé sur FilmCraft, par l'équipe ArtCraft et ses contributeurs.";

/// RECut's source repository.
pub const REPOSITORY: &str = "https://github.com/Foxnoxxx/filmcraft";

/// The red of the recording dot.
pub const REC_RED: egui::Color32 = egui::Color32::from_rgb(0xD7, 0x26, 0x2E);

fn ink(dark: bool) -> egui::Color32 {
    if dark { egui::Color32::from_rgb(0xF2, 0xF2, 0xF2) } else { egui::Color32::from_rgb(0x11, 0x11, 0x11) }
}

fn layout(ui: &egui::Ui, height: f32, dark: bool) -> std::sync::Arc<egui::Galley> {
    ui.painter().layout_no_wrap(APP_NAME.to_string(), crate::theme::Tokens::semibold(height.max(1.0)), ink(dark))
}

/// Width in points of the wordmark drawn `height` points tall.
pub fn wordmark_width(ui: &egui::Ui, height: f32) -> f32 {
    let galley = layout(ui, height, true);
    height * 0.9 + galley.size().x
}

/// Draw the wordmark `height` points tall with its left edge at `left_center` (vertically centred).
/// Returns the drawn rect.
pub fn paint_wordmark(ui: &egui::Ui, left_center: egui::Pos2, height: f32, dark: bool) -> Option<egui::Rect> {
    let height = height.max(1.0);
    let radius = height * 0.3;
    let painter = ui.painter();
    painter.circle_filled(egui::pos2(left_center.x + radius, left_center.y), radius, REC_RED);
    let galley = layout(ui, height, dark);
    let size = galley.size();
    let text_x = left_center.x + height * 0.9;
    painter.galley(egui::pos2(text_x, left_center.y - size.y / 2.0), galley, ink(dark));
    let width = text_x + size.x - left_center.x;
    Some(egui::Rect::from_min_size(egui::pos2(left_center.x, left_center.y - height / 2.0), egui::vec2(width, height)))
}
