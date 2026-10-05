//! AQW memory monitor (F6): a small corner panel with live memory readings and
//! a "Clean memory" button.

use egui::{Align2, Area, Color32, CornerRadius, Frame, Id, Margin, Order, RichText, vec2};
use ruffle_core::Player;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const SAMPLE_EVERY: Duration = Duration::from_secs(1);
const TREND_WINDOW: usize = 60; // samples (≈ one minute)
const REPORT_AFTER: Duration = Duration::from_millis(1500);
const MESSAGE_FOR: Duration = Duration::from_secs(8);
const MB: f64 = 1024.0 * 1024.0;
const GB: f64 = 1024.0 * 1024.0 * 1024.0;

struct PendingReport {
    at: Instant,
    ram_before: Option<u64>,
    gpu_before: Option<u64>,
    released_internal: u64,
}

pub struct AqwMemoryPanel {
    visible: bool,
    last_sample: Option<Instant>,
    ram: Option<u64>,
    gpu: Option<(u64, u64)>,
    assets: usize,
    peak_ram: u64,
    history: VecDeque<u64>,
    message: Option<(String, Instant)>,
    pending: Option<PendingReport>,
}

impl Default for AqwMemoryPanel {
    fn default() -> Self {
        Self {
            visible: std::env::var("RUFFLE_AQW_MEMORY_PANEL")
                .is_ok_and(|v| !matches!(v.trim(), "" | "0" | "false" | "off")),
            last_sample: None,
            ram: None,
            gpu: None,
            assets: 0,
            peak_ram: 0,
            history: VecDeque::with_capacity(TREND_WINDOW + 1),
            message: None,
            pending: None,
        }
    }
}

fn fmt_bytes(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else {
        format!("{:.0} MB", b / MB)
    }
}

fn fmt_delta(delta: i64) -> String {
    let sign = if delta < 0 { "-" } else { "+" };
    format!("{sign}{:.0} MB", delta.unsigned_abs() as f64 / MB)
}

impl AqwMemoryPanel {
    pub fn toggle(&mut self) {
        self.visible = !self.visible;
        self.last_sample = None;
    }

    fn sample(&mut self, now: Instant) {
        self.ram = ruffle_render_wgpu::backend::aqw_process_memory_bytes();
        self.gpu = ruffle_render_wgpu::backend::aqw_gpu_memory_bytes();
        self.assets = ruffle_frontend_utils::backends::navigator::aqw_asset_memory_bytes();
        if let Some(ram) = self.ram {
            self.peak_ram = self.peak_ram.max(ram);
            self.history.push_back(ram);
            while self.history.len() > TREND_WINDOW {
                self.history.pop_front();
            }
        }
        self.last_sample = Some(now);

        if let Some(pending) = &self.pending
            && now >= pending.at
        {
            let pending = self.pending.take().expect("checked above");
            let mut parts = Vec::new();
            if let (Some(before), Some(after)) = (pending.ram_before, self.ram) {
                parts.push(format!("RAM {}", fmt_delta(after as i64 - before as i64)));
            }
            if let (Some(before), Some((after, _))) = (pending.gpu_before, self.gpu) {
                parts.push(format!("graphics {}", fmt_delta(after as i64 - before as i64)));
            }
            let measured = if parts.is_empty() {
                String::new()
            } else {
                format!(" ({})", parts.join(", "))
            };
            self.message = Some((
                format!(
                    "Released {} internally{measured}",
                    fmt_bytes(pending.released_internal)
                ),
                now,
            ));
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, player: Option<&mut Player>) {
        if !self.visible {
            return;
        }
        let now = Instant::now();
        if self
            .last_sample
            .is_none_or(|at| now.duration_since(at) >= SAMPLE_EVERY)
        {
            self.sample(now);
        }
        if self
            .message
            .as_ref()
            .is_some_and(|(_, at)| now.duration_since(*at) > MESSAGE_FOR)
        {
            self.message = None;
        }

        let mut clean_clicked = false;
        Area::new(Id::new("aqw_memory_panel"))
            .order(Order::Foreground)
            .anchor(Align2::RIGHT_TOP, vec2(-10.0, 10.0))
            .show(ctx, |ui| {
                Frame::NONE
                    .fill(Color32::from_black_alpha(190))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_max_width(260.0);
                        let text = |s: String| RichText::new(s).color(Color32::WHITE).size(13.0);
                        let dim = |s: &str| RichText::new(s).color(Color32::GRAY).size(11.0);

                        ui.label(RichText::new("Memory").color(Color32::WHITE).strong());
                        match self.ram {
                            Some(ram) => {
                                ui.label(text(format!("Game RAM: {}", fmt_bytes(ram))));
                                ui.label(dim(&format!("peak {}", fmt_bytes(self.peak_ram))));
                                if self.history.len() >= 10
                                    && let Some(oldest) = self.history.front()
                                {
                                    let delta = ram as i64 - *oldest as i64;
                                    let secs = self.history.len();
                                    let color = if delta > 50 * 1024 * 1024 {
                                        Color32::from_rgb(255, 170, 90)
                                    } else {
                                        Color32::from_rgb(140, 220, 140)
                                    };
                                    ui.label(
                                        RichText::new(format!(
                                            "{} in the last {secs}s",
                                            fmt_delta(delta)
                                        ))
                                        .color(color)
                                        .size(11.0),
                                    );
                                }
                            }
                            None => {
                                ui.label(dim("Game RAM: not available"));
                            }
                        }
                        if let Some((used, budget)) = self.gpu {
                            ui.label(text(format!(
                                "Graphics: {} / {}",
                                fmt_bytes(used),
                                fmt_bytes(budget)
                            )));
                        }
                        ui.label(text(format!(
                            "Download cache: {}",
                            fmt_bytes(self.assets as u64)
                        )));

                        ui.add_space(4.0);
                        let busy = self.pending.is_some();
                        if ui
                            .add_enabled(!busy, egui::Button::new("Clean memory"))
                            .clicked()
                        {
                            clean_clicked = true;
                        }
                        if busy {
                            ui.label(dim("Cleaning..."));
                        } else if let Some((message, _)) = &self.message {
                            ui.label(
                                RichText::new(message)
                                    .color(Color32::from_rgb(140, 220, 140))
                                    .size(11.0),
                            );
                        }
                        ui.label(dim("F6 to hide"));
                    });
            });

        if clean_clicked {
            let ram_before = ruffle_render_wgpu::backend::aqw_process_memory_bytes();
            let gpu_before =
                ruffle_render_wgpu::backend::aqw_gpu_memory_bytes().map(|(used, _)| used);
            let mut released = 0u64;
            if let Some(player) = player {
                let (caches, pools) = player.aqw_clean_memory();
                released += caches + pools;
            }
            released += ruffle_frontend_utils::backends::navigator::clear_aqw_asset_memory() as u64;
            self.pending = Some(PendingReport {
                at: now + REPORT_AFTER,
                ram_before,
                gpu_before,
                released_internal: released,
            });
            self.message = None;
            // Re-sample right after the report delay.
            self.last_sample = Some(now + REPORT_AFTER - SAMPLE_EVERY);
        }

        ctx.request_repaint_after(SAMPLE_EVERY);
    }
}
