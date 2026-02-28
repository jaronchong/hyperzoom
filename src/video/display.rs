use std::collections::HashMap;

use eframe::egui;

use super::frame::VideoFrame;

/// Manages per-participant egui textures and renders a 2x2 video grid.
pub struct VideoDisplay {
    local_texture: Option<egui::TextureHandle>,
    remote_textures: HashMap<u8, egui::TextureHandle>,
}

impl VideoDisplay {
    pub fn new() -> Self {
        Self {
            local_texture: None,
            remote_textures: HashMap::new(),
        }
    }

    /// Upload the local camera preview frame to a GPU texture.
    pub fn update_local(&mut self, ctx: &egui::Context, frame: &VideoFrame) {
        let image = egui::ColorImage::from_rgb(
            [frame.width as usize, frame.height as usize],
            &frame.data,
        );
        match &mut self.local_texture {
            Some(tex) => tex.set(image, egui::TextureOptions::LINEAR),
            None => {
                let tex = ctx.load_texture("local_video", image, egui::TextureOptions::LINEAR);
                self.local_texture = Some(tex);
            }
        }
    }

    /// Upload a decoded remote peer frame to a GPU texture.
    pub fn update_remote(
        &mut self,
        ctx: &egui::Context,
        participant_id: u8,
        frame: &VideoFrame,
    ) {
        let image = egui::ColorImage::from_rgb(
            [frame.width as usize, frame.height as usize],
            &frame.data,
        );
        match self.remote_textures.get_mut(&participant_id) {
            Some(tex) => tex.set(image, egui::TextureOptions::LINEAR),
            None => {
                let name = format!("remote_video_{participant_id}");
                let tex = ctx.load_texture(name, image, egui::TextureOptions::LINEAR);
                self.remote_textures.insert(participant_id, tex);
            }
        }
    }

    /// Render the 2x2 video grid: local preview + up to 3 remote feeds.
    pub fn show_grid(
        &self,
        ui: &mut egui::Ui,
        local_name: &str,
        peers: &[(u8, String)], // (participant_id, name) sorted by id
    ) {
        let available = ui.available_size();
        let cell_w = (available.x - 8.0) / 2.0; // 8px gap
        let cell_h = (available.y - 8.0) / 2.0;
        let cell_size = egui::vec2(cell_w, cell_h);

        // Build list of cells: [local, peer0, peer1, peer2]
        // Up to 4 cells in a 2x2 grid
        let total_cells = 1 + peers.len().min(3);

        egui::Grid::new("video_grid")
            .num_columns(2)
            .spacing(egui::vec2(4.0, 4.0))
            .show(ui, |ui| {
                for cell_idx in 0..4 {
                    if cell_idx == 2 {
                        ui.end_row();
                    }

                    if cell_idx == 0 {
                        // Local preview
                        self.show_cell(ui, cell_size, &self.local_texture, local_name);
                    } else if cell_idx - 1 < peers.len().min(3) {
                        let (pid, ref name) = peers[cell_idx - 1];
                        let tex = self.remote_textures.get(&pid);
                        self.show_cell(ui, cell_size, &tex.cloned(), name);
                    } else if cell_idx < total_cells.max(4) {
                        // Empty placeholder
                        self.show_placeholder(ui, cell_size, "");
                    }
                }
            });
    }

    fn show_cell(
        &self,
        ui: &mut egui::Ui,
        size: egui::Vec2,
        texture: &Option<egui::TextureHandle>,
        name: &str,
    ) {
        let (rect, _response) = ui.allocate_exact_size(size, egui::Sense::hover());

        // Dark background
        ui.painter()
            .rect_filled(rect, 4.0, egui::Color32::from_gray(30));

        if let Some(tex) = texture {
            // Draw video frame, maintaining aspect ratio within the cell
            let tex_size = tex.size_vec2();
            let scale = (rect.width() / tex_size.x).min(rect.height() / tex_size.y);
            let img_size = tex_size * scale;
            let img_rect = egui::Rect::from_center_size(rect.center(), img_size);

            ui.painter().image(
                tex.id(),
                img_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }

        // Name overlay at bottom
        if !name.is_empty() {
            let text_pos = egui::pos2(rect.left() + 8.0, rect.bottom() - 24.0);
            // Background for text readability
            let text_bg = egui::Rect::from_min_size(
                egui::pos2(rect.left(), rect.bottom() - 28.0),
                egui::vec2(rect.width(), 28.0),
            );
            ui.painter()
                .rect_filled(text_bg, 0.0, egui::Color32::from_black_alpha(128));
            ui.painter().text(
                text_pos,
                egui::Align2::LEFT_CENTER,
                name,
                egui::FontId::proportional(14.0),
                egui::Color32::WHITE,
            );
        }
    }

    fn show_placeholder(&self, ui: &mut egui::Ui, size: egui::Vec2, label: &str) {
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, 4.0, egui::Color32::from_gray(20));
        if !label.is_empty() {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(16.0),
                egui::Color32::from_gray(100),
            );
        }
    }
}
