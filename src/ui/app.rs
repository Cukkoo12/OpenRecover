use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

use eframe::egui::{self, Color32, CornerRadius, RichText, Stroke, TextureHandle, Vec2};
use egui_extras::{Column, TableBuilder};

use crate::core::device::{
    enumerate_devices, inspect_image_file, is_elevated, is_same_volume, open_device_reader,
    relaunch_as_admin, BlockDevice,
};
use crate::core::recover::{read_entry_bytes, recover_entries};
use crate::core::types::{
    format_bytes, BusKind, DeviceInfo, FileCategory, FileHealth, RecoverEvent, RecoverSummary,
    RecoveredEntry, ScanEvent, ScanMode,
};
use crate::core::{run_scan, FolderNode};
use crate::i18n::Lang;

// Warm Orange + Crisp White Design System
const BG_CANVAS: Color32 = Color32::from_rgb(250, 249, 247);
const BG_SURFACE: Color32 = Color32::from_rgb(255, 255, 255);
const BG_CARD: Color32 = Color32::from_rgb(255, 255, 255);
const BG_SUBTLE: Color32 = Color32::from_rgb(244, 243, 241);
const BG_CARD_HOVER: Color32 = Color32::from_rgb(255, 243, 232);
const BORDER_SUBTLE: Color32 = Color32::from_rgb(218, 213, 206);
const ACCENT: Color32 = Color32::from_rgb(234, 88, 12);
const ACCENT_SOFT: Color32 = Color32::from_rgb(255, 241, 229);
const ACCENT_BORDER: Color32 = Color32::from_rgb(253, 186, 116);
const EMERALD: Color32 = Color32::from_rgb(5, 150, 105);
const EMERALD_SOFT: Color32 = Color32::from_rgb(232, 250, 242);
const AMBER: Color32 = Color32::from_rgb(180, 83, 9);
const AMBER_SOFT: Color32 = Color32::from_rgb(254, 245, 224);
const RED: Color32 = Color32::from_rgb(220, 38, 38);
const RED_SOFT: Color32 = Color32::from_rgb(254, 236, 236);
const TEXT_PRI: Color32 = Color32::from_rgb(24, 24, 27);
const TEXT_SEC: Color32 = Color32::from_rgb(82, 82, 91);
const TEXT_MUTED: Color32 = Color32::from_rgb(132, 132, 143);

enum PreviewData {
    None,
    Image {
        texture: TextureHandle,
        dims: (u32, u32),
    },
    Text {
        text: String,
    },
    Hex {
        hex: String,
    },
    Error(String),
}

pub struct OpenRecoverApp {
    lang: Lang,
    is_admin: bool,
    elevate_failed: bool,
    show_home: bool,
    show_folder_tree: bool,
    pending_auto_scan: bool,
    pending_preview_first: bool,

    devices: Vec<DeviceInfo>,
    selected_device_idx: Option<usize>,
    show_physical_disks: bool,
    scan_mode: ScanMode,

    // Scan state
    scanning: bool,
    scan_cancel: Arc<AtomicBool>,
    scan_rx: Option<mpsc::Receiver<ScanEvent>>,
    scan_stage: String,
    scan_fraction: f32,
    scan_found: usize,
    scan_detail: String,
    scan_elapsed: Option<f64>,
    scan_warnings: Vec<String>,
    scan_error: Option<String>,

    // Results & filtering
    active_reader: Option<Arc<dyn BlockDevice>>,
    entries: Vec<RecoveredEntry>,
    selected: Vec<bool>,
    folder_tree: FolderNode,
    active_folder_filter: Option<String>,
    search_query: String,
    category_filter: FileCategory,
    hide_trimmed: bool,
    filtered_indices: Vec<usize>,
    filters_dirty: bool,

    // Selection stats
    selected_count: usize,
    selected_bytes: u64,

    // Preview state
    focused_entry_idx: Option<usize>,
    preview: PreviewData,

    // Recovery state
    dest_dir: String,
    separate_trimmed: bool,
    recovering: bool,
    recover_cancel: Arc<AtomicBool>,
    recover_rx: Option<mpsc::Receiver<RecoverEvent>>,
    recover_done: usize,
    recover_total: usize,
    recover_bytes: u64,
    recover_current: String,
    recover_summary: Option<RecoverSummary>,
    recover_error: Option<String>,
}

impl OpenRecoverApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        load_system_fonts(&cc.egui_ctx);
        apply_theme(&cc.egui_ctx);

        let mut devices = enumerate_devices();
        let mut selected_idx = devices
            .iter()
            .position(|d| d.is_removable && !d.is_physical_partition)
            .or_else(|| (!devices.is_empty()).then_some(0));

        let mut pending_auto_scan = false;
        let mut pending_preview_first = false;
        let mut initial_lang = Lang::Tr;
        for arg in std::env::args().skip(1) {
            if arg == "--scan" {
                pending_auto_scan = true;
            } else if arg == "--preview-first" {
                pending_preview_first = true;
            } else if arg == "--en" {
                initial_lang = Lang::En;
            } else if !arg.starts_with("--") {
                let p = PathBuf::from(&arg);
                if p.exists() && p.is_file() {
                    if let Ok(imgs) = inspect_image_file(&p) {
                        if !imgs.is_empty() {
                            let first_new = devices.len();
                            devices.extend(imgs);
                            selected_idx = Some(first_new);
                        }
                    }
                }
            }
        }

        Self {
            lang: initial_lang,
            is_admin: is_elevated(),
            elevate_failed: false,
            show_home: true,
            show_folder_tree: true,
            pending_auto_scan,
            pending_preview_first,

            devices,
            selected_device_idx: selected_idx,
            show_physical_disks: false,
            scan_mode: ScanMode::QuickTree,

            scanning: false,
            scan_cancel: Arc::new(AtomicBool::new(false)),
            scan_rx: None,
            scan_stage: String::new(),
            scan_fraction: 0.0,
            scan_found: 0,
            scan_detail: String::new(),
            scan_elapsed: None,
            scan_warnings: Vec::new(),
            scan_error: None,

            active_reader: None,
            entries: Vec::new(),
            selected: Vec::new(),
            folder_tree: FolderNode::new("Root".to_string(), String::new()),
            active_folder_filter: None,
            search_query: String::new(),
            category_filter: FileCategory::All,
            hide_trimmed: false,
            filtered_indices: Vec::new(),
            filters_dirty: false,

            selected_count: 0,
            selected_bytes: 0,

            focused_entry_idx: None,
            preview: PreviewData::None,

            dest_dir: String::new(),
            separate_trimmed: true,
            recovering: false,
            recover_cancel: Arc::new(AtomicBool::new(false)),
            recover_rx: None,
            recover_done: 0,
            recover_total: 0,
            recover_bytes: 0,
            recover_current: String::new(),
            recover_summary: None,
            recover_error: None,
        }
    }

    fn refresh_devices(&mut self) {
        let images: Vec<DeviceInfo> = self
            .devices
            .iter()
            .filter(|d| d.bus_kind == BusKind::ImageFile)
            .cloned()
            .collect();
        self.devices = enumerate_devices();
        self.devices.extend(images);
        if self.selected_device_idx.is_none_or(|i| i >= self.devices.len()) {
            self.selected_device_idx = (!self.devices.is_empty()).then_some(0);
        }
    }

    fn open_image_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Disk Images", &["img", "dd", "raw", "bin"])
            .pick_file()
        {
            if let Ok(imgs) = inspect_image_file(&path) {
                let first_new = self.devices.len();
                self.devices.extend(imgs);
                if first_new < self.devices.len() {
                    self.selected_device_idx = Some(first_new);
                    self.show_home = true;
                }
            }
        }
    }

    fn start_scan(&mut self, ctx: &egui::Context) {
        let Some(dev_idx) = self.selected_device_idx else {
            return;
        };
        let device = self.devices[dev_idx].clone();
        let mode = self.scan_mode;

        self.show_home = false;
        self.scanning = true;
        self.scan_error = None;
        self.scan_warnings.clear();
        self.scan_elapsed = None;
        self.recover_summary = None;
        self.recover_error = None;
        self.scan_stage = self.lang.starting_stage().to_string();
        self.scan_fraction = 0.0;
        self.scan_found = 0;
        self.scan_detail.clear();
        self.entries.clear();
        self.selected.clear();
        self.filtered_indices.clear();
        self.selected_count = 0;
        self.selected_bytes = 0;
        self.focused_entry_idx = None;
        self.preview = PreviewData::None;
        self.active_folder_filter = None;

        self.active_reader = open_device_reader(&device).ok();

        let cancel = Arc::new(AtomicBool::new(false));
        self.scan_cancel = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();
        self.scan_rx = Some(rx);
        let ctx_clone = ctx.clone();

        std::thread::spawn(move || {
            let final_ev = run_scan(&device, mode, &cancel, &mut |ev| {
                let _ = tx.send(ev);
                ctx_clone.request_repaint();
            });
            let _ = tx.send(final_ev);
            ctx_clone.request_repaint();
        });
    }

    fn poll_scan(&mut self) {
        let mut done = false;
        if let Some(ref rx) = self.scan_rx {
            while let Ok(ev) = rx.try_recv() {
                match ev {
                    ScanEvent::Progress {
                        stage,
                        fraction,
                        found_count,
                        detail,
                    } => {
                        self.scan_stage = stage;
                        self.scan_fraction = fraction.clamp(0.0, 1.0);
                        self.scan_found = found_count;
                        self.scan_detail = detail;
                    }
                    ScanEvent::Finished {
                        entries,
                        elapsed_secs,
                        warnings,
                    } => {
                        self.scanning = false;
                        self.scan_fraction = 1.0;
                        self.scan_found = entries.len();
                        self.scan_elapsed = Some(elapsed_secs);
                        self.scan_warnings = warnings;
                        self.selected = vec![false; entries.len()];
                        self.folder_tree = FolderNode::build(&entries);
                        self.entries = entries;
                        self.filters_dirty = true;
                        done = true;
                    }
                    ScanEvent::Failed(err) => {
                        self.scanning = false;
                        self.scan_error = Some(err);
                        done = true;
                    }
                }
            }
        }
        if done {
            self.scan_rx = None;
        }
    }

    fn start_recovery(&mut self, ctx: &egui::Context) {
        let Some(dev_idx) = self.selected_device_idx else {
            return;
        };
        let device = &self.devices[dev_idx];
        let dest = PathBuf::from(self.dest_dir.trim());
        if self.dest_dir.trim().is_empty() || is_same_volume(device, &dest) {
            return;
        }
        let chosen: Vec<RecoveredEntry> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| self.selected.get(i).copied().unwrap_or(false).then(|| e.clone()))
            .collect();
        if chosen.is_empty() {
            return;
        }
        let reader = match self
            .active_reader
            .clone()
            .or_else(|| open_device_reader(device).ok())
        {
            Some(r) => r,
            None => {
                self.recover_error = Some(self.lang.err_open_reader().to_string());
                return;
            }
        };

        self.recovering = true;
        self.recover_summary = None;
        self.recover_error = None;
        self.recover_done = 0;
        self.recover_total = chosen.len();
        self.recover_bytes = 0;
        self.recover_current.clear();

        let cancel = Arc::new(AtomicBool::new(false));
        self.recover_cancel = Arc::clone(&cancel);
        let sep = self.separate_trimmed;
        let (tx, rx) = mpsc::channel();
        self.recover_rx = Some(rx);
        let ctx_clone = ctx.clone();

        std::thread::spawn(move || {
            let summary = recover_entries(&reader, &chosen, &dest, sep, &cancel, &mut |ev| {
                let _ = tx.send(ev);
                ctx_clone.request_repaint();
            });
            let _ = tx.send(RecoverEvent::Finished(summary));
            ctx_clone.request_repaint();
        });
    }

    fn poll_recovery(&mut self) {
        let mut done = false;
        if let Some(ref rx) = self.recover_rx {
            while let Ok(ev) = rx.try_recv() {
                match ev {
                    RecoverEvent::Progress {
                        done_files,
                        total_files,
                        bytes_written,
                        current_file,
                    } => {
                        self.recover_done = done_files;
                        self.recover_total = total_files;
                        self.recover_bytes = bytes_written;
                        self.recover_current = current_file;
                    }
                    RecoverEvent::Finished(summary) => {
                        self.recovering = false;
                        self.recover_summary = Some(summary);
                        done = true;
                    }
                    RecoverEvent::Failed(err) => {
                        self.recovering = false;
                        self.recover_error = Some(err);
                        done = true;
                    }
                }
            }
        }
        if done {
            self.recover_rx = None;
        }
    }

    fn apply_filters(&mut self) {
        self.filters_dirty = false;
        let q = self.search_query.trim().to_ascii_lowercase().replace('\\', "/");
        let folder = self.active_folder_filter.as_deref();

        self.filtered_indices.clear();
        for (i, e) in self.entries.iter().enumerate() {
            if self.hide_trimmed && e.health == FileHealth::LikelyTrimmedZero {
                continue;
            }
            if self.category_filter != FileCategory::All && e.category != self.category_filter {
                continue;
            }
            if let Some(fp) = folder {
                if !fp.is_empty()
                    && e.folder_path != fp
                    && !e.folder_path.starts_with(&format!("{}/", fp))
                {
                    continue;
                }
            }
            if !q.is_empty() && !e.rel_path_lower.contains(&q) {
                continue;
            }
            self.filtered_indices.push(i);
        }
    }

    fn recompute_selection_stats(&mut self) {
        let mut cnt = 0usize;
        let mut bytes = 0u64;
        for (i, &sel) in self.selected.iter().enumerate() {
            if sel {
                cnt += 1;
                bytes += self.entries[i].size;
            }
        }
        self.selected_count = cnt;
        self.selected_bytes = bytes;
    }

    fn load_preview(&mut self, ctx: &egui::Context, idx: usize) {
        self.focused_entry_idx = Some(idx);
        let Some(entry) = self.entries.get(idx).cloned() else {
            self.preview = PreviewData::None;
            return;
        };
        let Some(ref reader) = self.active_reader else {
            self.preview = PreviewData::Error(self.lang.err_no_reader().to_string());
            return;
        };

        let is_img = matches!(
            entry.ext.as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp"
        );
        let max_read = if is_img { 12 * 1024 * 1024 } else { 16 * 1024 };

        match read_entry_bytes(reader.as_ref(), &entry, max_read) {
            Ok(bytes) => {
                if is_img && !bytes.is_empty() && entry.size <= 16 * 1024 * 1024 {
                    if let Ok(img) = image::load_from_memory(&bytes) {
                        let rgba = img.to_rgba8();
                        let dims = rgba.dimensions();
                        let color_img = egui::ColorImage::from_rgba_unmultiplied(
                            [dims.0 as usize, dims.1 as usize],
                            rgba.as_raw(),
                        );
                        let tex = ctx.load_texture(
                            format!("preview_{}", entry.id),
                            color_img,
                            egui::TextureOptions::LINEAR,
                        );
                        self.preview = PreviewData::Image { texture: tex, dims };
                        return;
                    }
                }

                let is_text_ext = matches!(
                    entry.ext.as_str(),
                    "txt"
                        | "json"
                        | "md"
                        | "log"
                        | "ini"
                        | "cfg"
                        | "conf"
                        | "xml"
                        | "csv"
                        | "yaml"
                        | "yml"
                        | "toml"
                        | "rs"
                        | "py"
                        | "js"
                        | "ts"
                        | "cs"
                        | "gd"
                        | "html"
                        | "css"
                        | "sql"
                );
                if is_text_ext
                    || (!bytes.is_empty()
                        && bytes
                            .iter()
                            .take(256)
                            .all(|&b| b == 9 || b == 10 || b == 13 || (0x20..0x7F).contains(&b)))
                {
                    if let Ok(s) = std::str::from_utf8(&bytes) {
                        self.preview = PreviewData::Text {
                            text: s.chars().take(4000).collect(),
                        };
                        return;
                    }
                }

                let hex = format_hex_dump(&bytes[..bytes.len().min(512)]);
                self.preview = PreviewData::Hex { hex };
            }
            Err(e) => {
                self.preview = PreviewData::Error(e.to_string());
            }
        }
    }

    fn render_folder_node(&mut self, ui: &mut egui::Ui, node: &FolderNode) {
        let mut indices = Vec::new();
        node.collect_all_indices(&mut indices);
        let checked_count = indices
            .iter()
            .filter(|&&i| self.selected.get(i).copied().unwrap_or(false))
            .count();
        let mut all_checked = !indices.is_empty() && checked_count == indices.len();

        let is_active = self.active_folder_filter.as_deref() == Some(node.path.as_str());
        let name_color = if is_active { ACCENT } else { TEXT_PRI };
        let display_name = if node.path.is_empty() {
            self.lang.root_folder_name()
        } else {
            node.name.as_str()
        };

        if node.children.is_empty() {
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                if ui.checkbox(&mut all_checked, "").changed() {
                    for i in indices {
                        if let Some(slot) = self.selected.get_mut(i) {
                            *slot = all_checked;
                        }
                    }
                    self.recompute_selection_stats();
                }
                if ui
                    .selectable_label(
                        is_active,
                        RichText::new(format!("{}  {}", display_name, node.total_files))
                            .size(12.0)
                            .color(name_color),
                    )
                    .clicked()
                {
                    self.active_folder_filter = Some(node.path.clone());
                    self.filters_dirty = true;
                }
            });
        } else {
            let id = ui.make_persistent_id(format!("fld_{}", node.path));
            let state = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                id,
                node.path.is_empty(),
            );
            state
                .show_header(ui, |ui| {
                    if ui.checkbox(&mut all_checked, "").changed() {
                        for i in indices {
                            if let Some(slot) = self.selected.get_mut(i) {
                                *slot = all_checked;
                            }
                        }
                        self.recompute_selection_stats();
                    }
                    if ui
                        .selectable_label(
                            is_active,
                            RichText::new(format!("{}  {}", display_name, node.total_files))
                                .size(12.0)
                                .color(name_color),
                        )
                        .clicked()
                    {
                        self.active_folder_filter = Some(node.path.clone());
                        self.filters_dirty = true;
                    }
                })
                .body(|ui| {
                    for child in node.children.values() {
                        self.render_folder_node(ui, child);
                    }
                });
        }
    }
}

impl eframe::App for OpenRecoverApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        if self.pending_auto_scan {
            self.pending_auto_scan = false;
            self.start_scan(ctx);
        }
        self.poll_scan();
        self.poll_recovery();
        if self.filters_dirty {
            self.apply_filters();
        }
        if self.pending_preview_first && !self.scanning && !self.entries.is_empty() {
            self.pending_preview_first = false;
            let pick = self
                .entries
                .iter()
                .position(|e| matches!(e.ext.as_str(), "png" | "jpg" | "jpeg"))
                .unwrap_or(0);
            if let Some(slot) = self.selected.get_mut(pick) {
                *slot = true;
                self.recompute_selection_stats();
            }
            self.load_preview(ctx, pick);
        }

        // 1. MINIMALIST TOP APP BAR
        self.render_top_bar(ui, ctx);

        // 2. BOTTOM ACTION BAR (only shown in Results Explorer when files exist)
        if !self.show_home && !self.scanning && !self.entries.is_empty() {
            self.render_bottom_bar(ui, ctx);
        }

        // 3. CONTEXTUAL RIGHT PREVIEW DRAWER (only opens when user selects a file row)
        if !self.show_home && !self.scanning && self.focused_entry_idx.is_some() {
            self.render_preview_drawer(ui);
        }

        // 4. MAIN CANVAS: HOME / SCANNING / RESULTS
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(BG_CANVAS).inner_margin(egui::Margin::symmetric(18, 14)))
            .show(ui, |ui| {
                if self.scanning {
                    self.render_scanning_view(ui);
                } else if self.show_home {
                    self.render_home_view(ui, ctx);
                } else {
                    self.render_results_view(ui, ctx);
                }
            });
    }
}

impl OpenRecoverApp {
    fn render_top_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::Panel::top("top_bar")
            .frame(
                egui::Frame::NONE
                    .fill(BG_SURFACE)
                    .stroke(Stroke::new(1.0, BORDER_SUBTLE))
                    .inner_margin(egui::Margin::symmetric(18, 10)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    render_brand_mark(ui, 24.0);
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("OpenRecover")
                            .size(16.5)
                            .strong()
                            .color(TEXT_PRI),
                    );

                    if !self.show_home && !self.scanning {
                        ui.add_space(10.0);
                        if subtle_button(ui, self.lang.btn_back_sources()).clicked() {
                            self.show_home = true;
                        }
                        if let Some(dev) = self.selected_device_idx.and_then(|i| self.devices.get(i))
                        {
                            ui.add_space(6.0);
                            pill_badge(
                                ui,
                                &short_device_title(dev),
                                TEXT_SEC,
                                BG_SUBTLE,
                                BORDER_SUBTLE,
                            );
                        }
                    } else if self.show_home && !self.entries.is_empty() && !self.scanning {
                        ui.add_space(10.0);
                        if subtle_button(ui, self.lang.btn_back_results()).clicked() {
                            self.show_home = false;
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Segmented TR | EN switch
                        egui::Frame::NONE
                            .fill(BG_SUBTLE)
                            .stroke(Stroke::new(1.0, BORDER_SUBTLE))
                            .corner_radius(8.0)
                            .inner_margin(egui::Margin::same(2))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 2.0;
                                    if pill_button(ui, self.lang == Lang::En, "EN").clicked() {
                                        self.lang = Lang::En;
                                    }
                                    if pill_button(ui, self.lang == Lang::Tr, "TR").clicked() {
                                        self.lang = Lang::Tr;
                                    }
                                });
                            });

                        ui.add_space(6.0);

                        if self.is_admin {
                            pill_badge(
                                ui,
                                &format!("● {}", self.lang.title_admin_ok()),
                                EMERALD,
                                EMERALD_SOFT,
                                Color32::from_rgba_unmultiplied(16, 185, 129, 90),
                            );
                        } else if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(self.lang.btn_elevate())
                                        .size(12.0)
                                        .strong()
                                        .color(AMBER),
                                )
                                .fill(AMBER_SOFT)
                                .stroke(Stroke::new(1.0, Color32::from_rgba_unmultiplied(245, 158, 11, 100)))
                                .corner_radius(8.0),
                            )
                            .clicked()
                        {
                            if relaunch_as_admin().is_ok() {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            } else {
                                self.elevate_failed = true;
                                if !self.scanning && self.entries.is_empty() {
                                    self.show_home = true;
                                }
                            }
                        }

                        if !self.scanning {
                            if subtle_button(ui, self.lang.btn_open_img()).clicked() {
                                self.open_image_dialog();
                            }
                            if self.show_home && subtle_button(ui, self.lang.btn_refresh()).clicked()
                            {
                                self.refresh_devices();
                            }
                        }
                    });
                });
            });
    }

    fn render_home_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::ScrollArea::vertical().id_salt("home_scroll").show(ui, |ui| {
            let max_w = 860.0f32.min(ui.available_width() - 16.0).max(320.0);
            ui.vertical_centered(|ui| {
                ui.allocate_ui(Vec2::new(max_w, 0.0), |ui| {
                    ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                        ui.add_space(12.0);

                        // Centered hero title & calm subtitle
                        ui.vertical_centered(|ui| {
                            ui.label(
                                RichText::new(self.lang.home_title())
                                    .size(23.0)
                                    .strong()
                                    .color(TEXT_PRI),
                            );
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(self.lang.home_subtitle())
                                    .size(13.0)
                                    .color(TEXT_SEC),
                            );
                        });

                        if !self.is_admin {
                            ui.add_space(14.0);
                            let banner_outer = max_w;
                            egui::Frame::NONE
                                .fill(AMBER_SOFT)
                                .stroke(Stroke::new(1.0, Color32::from_rgba_unmultiplied(245, 158, 11, 90)))
                                .corner_radius(10.0)
                                .inner_margin(egui::Margin::symmetric(14, 9))
                                .show(ui, |ui| {
                                    ui.set_width((banner_outer - 28.0).max(120.0));
                                    let hint_text = if self.elevate_failed {
                                        self.lang.admin_elevate_hint()
                                    } else {
                                        self.lang.admin_hint()
                                    };
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(
                                            RichText::new(hint_text)
                                                .size(12.0)
                                                .color(AMBER),
                                        );
                                    });
                                });
                        }

                        ui.add_space(18.0);

                        // Group 1: Removable / USB / SD / Images
                        let removable_indices: Vec<usize> = self
                            .devices
                            .iter()
                            .enumerate()
                            .filter(|(_, d)| d.is_removable && !d.is_physical_partition)
                            .map(|(i, _)| i)
                            .collect();
                        if !removable_indices.is_empty() {
                            section_header(ui, self.lang.sec_removable());
                            self.render_device_grid(ui, &removable_indices);
                            ui.add_space(12.0);
                        }

                        // Group 2: Internal Fixed Drives
                        let fixed_indices: Vec<usize> = self
                            .devices
                            .iter()
                            .enumerate()
                            .filter(|(_, d)| !d.is_removable && !d.is_physical_partition)
                            .map(|(i, _)| i)
                            .collect();
                        section_header_with_right(ui, self.lang.sec_fixed(), |ui| {
                            ui.checkbox(
                                &mut self.show_physical_disks,
                                RichText::new(self.lang.sec_physical()).size(12.0).color(TEXT_SEC),
                            );
                        });
                        self.render_device_grid_with_image_card(ui, &fixed_indices, max_w);

                        if self.show_physical_disks {
                            let phys_indices: Vec<usize> = self
                                .devices
                                .iter()
                                .enumerate()
                                .filter(|(_, d)| d.is_physical_partition)
                                .map(|(i, _)| i)
                                .collect();
                            if !phys_indices.is_empty() {
                                ui.add_space(12.0);
                                section_header(
                                    ui,
                                    if self.lang.is_tr() {
                                        "Ham Fiziksel Bölümler"
                                    } else {
                                        "Raw Physical Partitions"
                                    },
                                );
                                self.render_device_grid(ui, &phys_indices);
                            }
                        }

                        ui.add_space(14.0);

                        // Scan Mode + Primary Action Card
                        egui::Frame::NONE
                            .fill(BG_SURFACE)
                            .stroke(Stroke::new(1.0, BORDER_SUBTLE))
                            .corner_radius(12.0)
                            .inner_margin(egui::Margin::symmetric(16, 14))
                            .show(ui, |ui| {
                                ui.set_width((max_w - 32.0).max(200.0));
                                ui.horizontal(|ui| {
                                    ui.vertical(|ui| {
                                        ui.label(
                                            RichText::new(self.lang.sec_scan_mode())
                                                .size(12.0)
                                                .strong()
                                                .color(TEXT_SEC),
                                        );
                                        ui.add_space(6.0);
                                        egui::Frame::NONE
                                            .fill(BG_SUBTLE)
                                            .stroke(Stroke::new(1.0, BORDER_SUBTLE))
                                            .corner_radius(9.0)
                                            .inner_margin(egui::Margin::same(3))
                                            .show(ui, |ui| {
                                                ui.horizontal(|ui| {
                                                    ui.spacing_mut().item_spacing.x = 4.0;
                                                    if pill_button(
                                                        ui,
                                                        self.scan_mode == ScanMode::QuickTree,
                                                        self.lang.mode_quick(),
                                                    )
                                                    .clicked()
                                                    {
                                                        self.scan_mode = ScanMode::QuickTree;
                                                    }
                                                    if pill_button(
                                                        ui,
                                                        self.scan_mode == ScanMode::DeepCarve,
                                                        self.lang.mode_carve(),
                                                    )
                                                    .clicked()
                                                    {
                                                        self.scan_mode = ScanMode::DeepCarve;
                                                    }
                                                    if pill_button(
                                                        ui,
                                                        self.scan_mode == ScanMode::Combined,
                                                        self.lang.mode_combined(),
                                                    )
                                                    .clicked()
                                                    {
                                                        self.scan_mode = ScanMode::Combined;
                                                    }
                                                });
                                            });
                                        ui.add_space(6.0);
                                        let hint = match self.scan_mode {
                                            ScanMode::QuickTree => self.lang.mode_quick_hint(),
                                            ScanMode::DeepCarve => self.lang.mode_carve_hint(),
                                            ScanMode::Combined => self.lang.mode_combined_hint(),
                                        };
                                        ui.label(RichText::new(hint).size(11.5).color(TEXT_MUTED));
                                    });

                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        let can_start = self.selected_device_idx.is_some() && !self.recovering;
                                        if primary_cta_button(
                                            ui,
                                            can_start,
                                            self.lang.btn_start_scan(),
                                            ACCENT,
                                            Vec2::new(178.0, 42.0),
                                        )
                                        .clicked()
                                        {
                                            self.start_scan(ctx);
                                        }
                                    });
                                });
                            });

                        if let Some(ref err) = self.scan_error {
                            ui.add_space(10.0);
                            egui::Frame::NONE
                                .fill(RED_SOFT)
                                .stroke(Stroke::new(1.0, Color32::from_rgba_unmultiplied(239, 68, 68, 90)))
                                .corner_radius(10.0)
                                .inner_margin(egui::Margin::symmetric(14, 10))
                                .show(ui, |ui| {
                                    ui.set_width((max_w - 28.0).max(120.0));
                                    ui.colored_label(RED, err);
                                });
                        }

                        ui.add_space(18.0);
                    });
                });
            });
        });
    }

    fn render_device_grid(&mut self, ui: &mut egui::Ui, indices: &[usize]) {
        for pair in indices.chunks(2) {
            ui.columns(2, |cols| {
                self.render_device_card(&mut cols[0], pair[0]);
                if let Some(&second) = pair.get(1) {
                    self.render_device_card(&mut cols[1], second);
                }
            });
            ui.add_space(8.0);
        }
    }

    fn render_device_grid_with_image_card(
        &mut self,
        ui: &mut egui::Ui,
        indices: &[usize],
        max_w: f32,
    ) {
        if indices.len().is_multiple_of(2) {
            self.render_device_grid(ui, indices);
            self.render_open_image_card_wide(ui, max_w);
        } else {
            let total_slots = indices.len() + 1;
            let rows = total_slots.div_ceil(2);
            for r in 0..rows {
                ui.columns(2, |cols| {
                    for c in 0..2 {
                        let pos = r * 2 + c;
                        if let Some(&dev_idx) = indices.get(pos) {
                            self.render_device_card(&mut cols[c], dev_idx);
                        } else if pos == indices.len() {
                            self.render_open_image_card_slot(&mut cols[c]);
                        }
                    }
                });
                ui.add_space(8.0);
            }
        }
    }

    fn render_device_card(&mut self, ui: &mut egui::Ui, idx: usize) {
        let dev = self.devices[idx].clone();
        let is_sel = self.selected_device_idx == Some(idx);
        let fill = if is_sel { ACCENT_SOFT } else { BG_CARD };
        let stroke = if is_sel {
            Stroke::new(1.5, ACCENT)
        } else {
            Stroke::new(1.0, BORDER_SUBTLE)
        };
        let card_outer_w = ui.available_width();

        let frame_resp = egui::Frame::NONE
            .fill(fill)
            .stroke(stroke)
            .corner_radius(11.0)
            .inner_margin(egui::Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.set_width((card_outer_w - 28.0).max(120.0));
                ui.set_min_height(74.0);

                ui.horizontal(|ui| {
                    let bus_color = match dev.bus_kind {
                        BusKind::Usb | BusKind::SdMmc => EMERALD,
                        BusKind::ImageFile => AMBER,
                        _ => ACCENT,
                    };
                    pill_badge(
                        ui,
                        dev.bus_kind.label(self.lang.is_tr()),
                        bus_color,
                        Color32::from_rgba_unmultiplied(bus_color.r(), bus_color.g(), bus_color.b(), 24),
                        Color32::from_rgba_unmultiplied(bus_color.r(), bus_color.g(), bus_color.b(), 70),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        pill_badge(
                            ui,
                            dev.fs_kind.label(self.lang.is_tr()),
                            TEXT_SEC,
                            BG_SUBTLE,
                            BORDER_SUBTLE,
                        );
                    });
                });

                ui.add_space(8.0);
                ui.label(
                    RichText::new(short_device_title(&dev))
                        .size(14.5)
                        .strong()
                        .color(TEXT_PRI),
                );
                ui.add_space(6.0);

                if dev.total_bytes > 0 && dev.free_bytes > 0 && dev.total_bytes >= dev.free_bytes {
                    let used = dev.total_bytes - dev.free_bytes;
                    let frac = (used as f64 / dev.total_bytes as f64) as f32;
                    ui.add(
                        egui::ProgressBar::new(frac)
                            .desired_height(5.0)
                            .corner_radius(3.0)
                            .fill(if is_sel { ACCENT } else { Color32::from_rgb(161, 161, 170) }),
                    );
                    ui.add_space(3.0);
                    ui.label(
                        RichText::new(format!(
                            "{} / {}",
                            format_bytes(used),
                            format_bytes(dev.total_bytes)
                        ))
                        .size(11.0)
                        .color(TEXT_MUTED),
                    );
                } else if dev.total_bytes > 0 {
                    ui.label(
                        RichText::new(format_bytes(dev.total_bytes))
                            .size(11.5)
                            .color(TEXT_MUTED),
                    );
                } else {
                    ui.label(RichText::new(&dev.raw_device_path).size(11.0).color(TEXT_MUTED));
                }
            });

        let click = frame_resp.response.interact(egui::Sense::click());
        if click.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if click.clicked() {
            self.selected_device_idx = Some(idx);
        }
    }

    fn render_open_image_card_slot(&mut self, ui: &mut egui::Ui) {
        let col_w = ui.available_width();
        let frame_resp = egui::Frame::NONE
            .fill(BG_SURFACE)
            .stroke(Stroke::new(1.0, BORDER_SUBTLE))
            .corner_radius(11.0)
            .inner_margin(egui::Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.set_width((col_w - 28.0).max(120.0));
                ui.set_min_height(74.0);
                ui.vertical_centered(|ui| {
                    ui.add_space(20.0);
                    ui.label(
                        RichText::new(self.lang.open_img_card())
                            .size(13.0)
                            .strong()
                            .color(ACCENT),
                    );
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(if self.lang.is_tr() {
                            "Yönetici izni olmadan doğrudan tarar"
                        } else {
                            "Opens directly without Admin rights"
                        })
                        .size(11.0)
                        .color(TEXT_MUTED),
                    );
                });
            });

        let click = frame_resp.response.interact(egui::Sense::click());
        if click.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if click.clicked() {
            self.open_image_dialog();
        }
    }

    fn render_open_image_card_wide(&mut self, ui: &mut egui::Ui, max_w: f32) {
        let frame_resp = egui::Frame::NONE
            .fill(BG_SURFACE)
            .stroke(Stroke::new(1.0, BORDER_SUBTLE))
            .corner_radius(11.0)
            .inner_margin(egui::Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.set_width((max_w - 32.0).max(120.0));
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(self.lang.open_img_card())
                            .size(13.0)
                            .strong()
                            .color(ACCENT),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(if self.lang.is_tr() {
                                "Yönetici izni olmadan doğrudan açılır"
                            } else {
                                "Opens directly without Admin rights"
                            })
                            .size(11.5)
                            .color(TEXT_MUTED),
                        );
                    });
                });
            });

        let click = frame_resp.response.interact(egui::Sense::click());
        if click.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if click.clicked() {
            self.open_image_dialog();
        }
    }

    fn render_scanning_view(&mut self, ui: &mut egui::Ui) {
        let avail = ui.available_size();
        ui.add_space((avail.y * 0.18).max(24.0));
        ui.vertical_centered(|ui| {
            let card_w = 500.0f32.min(avail.x - 24.0).max(300.0);
            egui::Frame::NONE
                .fill(BG_SURFACE)
                .stroke(Stroke::new(1.0, BORDER_SUBTLE))
                .corner_radius(14.0)
                .inner_margin(egui::Margin::same(24))
                .show(ui, |ui| {
                    ui.set_width(card_w);

                    if let Some(dev) = self.selected_device_idx.and_then(|i| self.devices.get(i)) {
                        ui.horizontal(|ui| {
                            pill_badge(ui, &short_device_title(dev), ACCENT, ACCENT_SOFT, BORDER_SUBTLE);
                            pill_badge(
                                ui,
                                dev.fs_kind.label(self.lang.is_tr()),
                                TEXT_SEC,
                                BG_SUBTLE,
                                BORDER_SUBTLE,
                            );
                        });
                        ui.add_space(12.0);
                    }

                    let stage_txt = self.lang.localize_stage(&self.scan_stage);
                    ui.label(
                        RichText::new(&stage_txt)
                            .size(18.0)
                            .strong()
                            .color(TEXT_PRI),
                    );
                    ui.add_space(14.0);

                    ui.horizontal(|ui| {
                        let bar_w = (ui.available_width() - 48.0).max(160.0);
                        ui.add(
                            egui::ProgressBar::new(self.scan_fraction)
                                .desired_width(bar_w)
                                .desired_height(10.0)
                                .corner_radius(5.0)
                                .fill(ACCENT),
                        );
                        ui.label(
                            RichText::new(format!("{:.0}%", self.scan_fraction * 100.0))
                                .size(12.5)
                                .strong()
                                .color(TEXT_PRI),
                        );
                    });
                    ui.add_space(14.0);

                    ui.horizontal(|ui| {
                        pill_badge(
                            ui,
                            &if self.lang.is_tr() {
                                format!("Bulunan: {} dosya", self.scan_found)
                            } else {
                                format!("Found: {} files", self.scan_found)
                            },
                            EMERALD,
                            EMERALD_SOFT,
                            Color32::from_rgba_unmultiplied(16, 185, 129, 70),
                        );
                        if !self.scan_detail.is_empty() {
                            ui.add_space(8.0);
                            let detail_txt = self.lang.localize_detail(&self.scan_detail);
                            ui.label(RichText::new(&detail_txt).size(12.0).color(TEXT_SEC));
                        }
                    });

                    ui.add_space(20.0);
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(self.lang.btn_stop_scan())
                                    .size(13.0)
                                    .strong()
                                    .color(RED),
                            )
                            .fill(RED_SOFT)
                            .stroke(Stroke::new(1.0, Color32::from_rgba_unmultiplied(239, 68, 68, 90)))
                            .corner_radius(8.0)
                            .min_size(Vec2::new(120.0, 34.0)),
                        )
                        .clicked()
                    {
                        self.scan_cancel.store(true, Ordering::Relaxed);
                    }
                });
        });
    }

    fn render_results_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.entries.is_empty() {
            let avail = ui.available_size();
            ui.add_space((avail.y * 0.16).max(20.0));
            ui.vertical_centered(|ui| {
                let card_w = 500.0f32.min(avail.x - 24.0).max(300.0);
                egui::Frame::NONE
                    .fill(BG_SURFACE)
                    .stroke(Stroke::new(1.0, BORDER_SUBTLE))
                    .corner_radius(14.0)
                    .inner_margin(egui::Margin::same(24))
                    .show(ui, |ui| {
                        ui.set_width(card_w);
                        if let Some(ref err) = self.scan_error {
                            ui.colored_label(RED, RichText::new(err).size(13.5).strong());
                            ui.add_space(10.0);
                        } else {
                            ui.label(
                                RichText::new(self.lang.empty_title())
                                    .size(18.0)
                                    .strong()
                                    .color(TEXT_PRI),
                            );
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new(self.lang.empty_hint())
                                    .size(12.5)
                                    .color(TEXT_SEC),
                            );
                        }
                        for w in &self.scan_warnings {
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new(self.lang.localize_warning(w))
                                    .size(11.5)
                                    .color(AMBER),
                            );
                        }
                        ui.add_space(16.0);
                        ui.horizontal(|ui| {
                            if subtle_button(ui, self.lang.btn_back_sources()).clicked() {
                                self.show_home = true;
                            }
                            if self.scan_mode == ScanMode::QuickTree
                                && primary_cta_button(
                                    ui,
                                    true,
                                    self.lang.mode_carve(),
                                    ACCENT,
                                    Vec2::new(140.0, 34.0),
                                )
                                .clicked()
                            {
                                self.scan_mode = ScanMode::DeepCarve;
                                self.start_scan(ctx);
                            }
                        });
                    });
            });
            return;
        }

        // Top Filter Toolbar — Row 1: Search & Action Controls
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.search_query)
                    .desired_width(235.0)
                    .margin(egui::Margin::symmetric(10, 6))
                    .hint_text(self.lang.search_hint()),
            );
            if resp.changed() {
                self.filters_dirty = true;
            }
            if !self.search_query.is_empty() && subtle_button(ui, "×").clicked() {
                self.search_query.clear();
                self.filters_dirty = true;
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if subtle_button(ui, self.lang.btn_clear_sel()).clicked() {
                    self.selected.fill(false);
                    self.recompute_selection_stats();
                }
                if subtle_button(ui, self.lang.btn_select_visible()).clicked() {
                    for &idx in &self.filtered_indices {
                        if let Some(slot) = self.selected.get_mut(idx) {
                            *slot = true;
                        }
                    }
                    self.recompute_selection_stats();
                }
                if pill_button(ui, self.hide_trimmed, self.lang.hide_trimmed()).clicked() {
                    self.hide_trimmed = !self.hide_trimmed;
                    self.filters_dirty = true;
                }
                if pill_button(ui, self.show_folder_tree, self.lang.btn_folders()).clicked() {
                    self.show_folder_tree = !self.show_folder_tree;
                }
            });
        });

        ui.add_space(6.0);

        // Row 2: Category Pills + Live Counter
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for cat in [
                FileCategory::All,
                FileCategory::Images,
                FileCategory::Documents,
                FileCategory::VideosAudio,
                FileCategory::Archives,
                FileCategory::CodeConfig,
                FileCategory::Other,
            ] {
                if pill_button(ui, self.category_filter == cat, cat.label(self.lang.is_tr()))
                    .clicked()
                {
                    self.category_filter = cat;
                    self.filters_dirty = true;
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let unit = if self.lang.is_tr() { "sn" } else { "s" };
                let count_txt = if let Some(secs) = self.scan_elapsed {
                    format!(
                        "{} / {} ({:.2} {})",
                        self.filtered_indices.len(),
                        self.entries.len(),
                        secs,
                        unit
                    )
                } else {
                    format!("{} / {}", self.filtered_indices.len(), self.entries.len())
                };
                ui.label(RichText::new(count_txt).size(11.5).color(TEXT_MUTED));
            });
        });

        let has_active_folder = self
            .active_folder_filter
            .as_ref()
            .is_some_and(|fp| !fp.is_empty());
        if has_active_folder || !self.scan_warnings.is_empty() {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if let Some(ref fp) = self.active_folder_filter {
                    if !fp.is_empty() {
                        pill_badge(ui, fp, ACCENT, ACCENT_SOFT, BORDER_SUBTLE);
                        if ui.small_button("×").clicked() {
                            self.active_folder_filter = None;
                            self.filters_dirty = true;
                        }
                    }
                }
                for w in &self.scan_warnings {
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(self.lang.localize_warning(w))
                            .size(11.5)
                            .color(AMBER),
                    );
                }
            });
        }

        ui.add_space(8.0);

        // Main Split Area: Folder Tree (optional) + Virtualized File Table Card
        let avail_h = ui.available_height().max(180.0);
        ui.horizontal_top(|ui| {
            if self.show_folder_tree {
                egui::Frame::NONE
                    .fill(BG_SURFACE)
                    .stroke(Stroke::new(1.0, BORDER_SUBTLE))
                    .corner_radius(10.0)
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.set_width(195.0);
                            ui.set_height((avail_h - 22.0).max(140.0));
                            ui.label(
                                RichText::new(self.lang.folders_title())
                                    .strong()
                                    .size(12.5)
                                    .color(TEXT_PRI),
                            );
                            ui.add_space(4.0);
                            ui.separator();
                            ui.add_space(4.0);
                            egui::ScrollArea::both().id_salt("tree_scroll").show(ui, |ui| {
                                let root_clone = self.folder_tree.clone();
                                self.render_folder_node(ui, &root_clone);
                            });
                        });
                    });

                ui.add_space(8.0);
            }

            // Virtualized File Table Card
            egui::Frame::NONE
                .fill(BG_SURFACE)
                .stroke(Stroke::new(1.0, BORDER_SUBTLE))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.set_width((ui.available_width() - 16.0).max(200.0));
                        ui.set_height((avail_h - 18.0).max(140.0));

                        let mut clicked_row: Option<usize> = None;
                        let mut toggled_selection = false;

                        TableBuilder::new(ui)
                            .striped(true)
                            .resizable(true)
                            .sense(egui::Sense::click())
                            .column(Column::exact(26.0))
                            .column(Column::initial(145.0).at_least(85.0))
                            .column(Column::exact(68.0))
                            .column(Column::exact(96.0))
                            .column(Column::exact(96.0))
                            .column(Column::remainder().at_least(80.0))
                            .header(28.0, |mut header| {
                                header.col(|ui| {
                                    ui.label("");
                                });
                                header.col(|ui| {
                                    ui.label(RichText::new(self.lang.col_name()).size(12.0).strong().color(TEXT_SEC));
                                });
                                header.col(|ui| {
                                    ui.label(RichText::new(self.lang.col_size()).size(12.0).strong().color(TEXT_SEC));
                                });
                                header.col(|ui| {
                                    ui.label(RichText::new(self.lang.col_health()).size(12.0).strong().color(TEXT_SEC));
                                });
                                header.col(|ui| {
                                    ui.label(RichText::new(self.lang.col_source()).size(12.0).strong().color(TEXT_SEC));
                                });
                                header.col(|ui| {
                                    ui.label(RichText::new(self.lang.col_path()).size(12.0).strong().color(TEXT_SEC));
                                });
                            })
                            .body(|body| {
                                body.rows(26.0, self.filtered_indices.len(), |mut row| {
                                    let row_i = row.index();
                                    let entry_idx = self.filtered_indices[row_i];
                                    let e = &self.entries[entry_idx];
                                    let is_focused = self.focused_entry_idx == Some(entry_idx);
                                    row.set_selected(is_focused);

                                    row.col(|ui| {
                                        let mut chk = self.selected.get(entry_idx).copied().unwrap_or(false);
                                        if ui.checkbox(&mut chk, "").changed() {
                                            if let Some(slot) = self.selected.get_mut(entry_idx) {
                                                *slot = chk;
                                            }
                                            toggled_selection = true;
                                        }
                                    });
                                    row.col(|ui| {
                                        ui.label(RichText::new(&e.name).size(12.5).color(TEXT_PRI));
                                    });
                                    row.col(|ui| {
                                        ui.label(RichText::new(format_bytes(e.size)).size(12.0).color(TEXT_SEC));
                                    });
                                    row.col(|ui| {
                                        ui.colored_label(
                                            health_color(e.health),
                                            RichText::new(format!("● {}", e.health.label(self.lang.is_tr()))).size(11.5),
                                        );
                                    });
                                    row.col(|ui| {
                                        let src_txt = self.lang.localize_source_desc(&e.source_desc);
                                        ui.label(RichText::new(&src_txt).size(11.5).color(TEXT_SEC));
                                    });
                                    row.col(|ui| {
                                        ui.label(RichText::new(&e.folder_path).size(11.5).color(TEXT_MUTED));
                                    });

                                    if row.response().clicked() {
                                        clicked_row = Some(entry_idx);
                                    }
                                });
                            });

                        if toggled_selection {
                            self.recompute_selection_stats();
                        }
                        if let Some(idx) = clicked_row {
                            self.load_preview(ctx, idx);
                        }
                    });
                });
        });
    }

    fn render_preview_drawer(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("preview_drawer")
            .default_size(285.0)
            .min_size(240.0)
            .max_size(460.0)
            .frame(
                egui::Frame::NONE
                    .fill(BG_SURFACE)
                    .stroke(Stroke::new(1.0, BORDER_SUBTLE))
                    .inner_margin(egui::Margin::same(14)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(self.lang.preview_title())
                            .size(14.0)
                            .strong()
                            .color(TEXT_PRI),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if subtle_button(ui, "×").clicked() {
                            self.focused_entry_idx = None;
                            self.preview = PreviewData::None;
                        }
                    });
                });

                ui.add_space(10.0);

                if let Some(idx) = self.focused_entry_idx {
                    if let Some(entry) = self.entries.get(idx) {
                        egui::Frame::NONE
                            .fill(BG_SUBTLE)
                            .stroke(Stroke::new(1.0, BORDER_SUBTLE))
                            .corner_radius(10.0)
                            .inner_margin(egui::Margin::same(12))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(
                                    RichText::new(&entry.name)
                                        .size(13.5)
                                        .strong()
                                        .color(TEXT_PRI),
                                );
                                ui.add_space(4.0);
                                ui.label(
                                    RichText::new(&entry.rel_path)
                                        .size(11.0)
                                        .color(TEXT_MUTED),
                                );
                                ui.add_space(8.0);
                                ui.horizontal_wrapped(|ui| {
                                    pill_badge(
                                        ui,
                                        &format_bytes(entry.size),
                                        TEXT_PRI,
                                        BG_SURFACE,
                                        BORDER_SUBTLE,
                                    );
                                    let hcol = health_color(entry.health);
                                    pill_badge(
                                        ui,
                                        entry.health.label(self.lang.is_tr()),
                                        hcol,
                                        Color32::from_rgba_unmultiplied(hcol.r(), hcol.g(), hcol.b(), 24),
                                        Color32::from_rgba_unmultiplied(hcol.r(), hcol.g(), hcol.b(), 70),
                                    );
                                    let src_txt = self.lang.localize_source_desc(&entry.source_desc);
                                    pill_badge(
                                        ui,
                                        &src_txt,
                                        TEXT_SEC,
                                        BG_SURFACE,
                                        BORDER_SUBTLE,
                                    );
                                });
                            });
                        ui.add_space(10.0);
                    }
                }

                egui::Frame::NONE
                    .fill(BG_CANVAS)
                    .stroke(Stroke::new(1.0, BORDER_SUBTLE))
                    .corner_radius(10.0)
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.set_min_height(ui.available_height().max(120.0));
                        egui::ScrollArea::vertical().id_salt("prev_scroll").show(ui, |ui| {
                            match &self.preview {
                                PreviewData::None => {}
                                PreviewData::Image { texture, dims } => {
                                    let avail_w = ui.available_width().max(120.0);
                                    let aspect = dims.1 as f32 / dims.0.max(1) as f32;
                                    let disp_h = (avail_w * aspect).min(280.0);
                                    let disp_w = (disp_h / aspect.max(0.01)).min(avail_w);
                                    ui.vertical_centered(|ui| {
                                        ui.image((texture.id(), Vec2::new(disp_w, disp_h)));
                                        ui.add_space(6.0);
                                        ui.label(
                                            RichText::new(format!("{} × {} px", dims.0, dims.1))
                                                .size(11.0)
                                                .color(TEXT_MUTED),
                                        );
                                    });
                                }
                                PreviewData::Text { text } => {
                                    ui.label(
                                        RichText::new(text)
                                            .monospace()
                                            .size(11.5)
                                            .color(TEXT_PRI),
                                    );
                                }
                                PreviewData::Hex { hex } => {
                                    ui.label(
                                        RichText::new(hex)
                                            .monospace()
                                            .size(11.0)
                                            .color(Color32::from_rgb(154, 52, 18)),
                                    );
                                }
                                PreviewData::Error(err) => {
                                    ui.colored_label(RED, err);
                                }
                            }
                        });
                    });
            });
    }

    fn render_bottom_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::Panel::bottom("bottom_bar")
            .frame(
                egui::Frame::NONE
                    .fill(BG_SURFACE)
                    .stroke(Stroke::new(1.0, BORDER_SUBTLE))
                    .inner_margin(egui::Margin::symmetric(18, 10)),
            )
            .show(ui, |ui| {
                let same_vol = self
                    .selected_device_idx
                    .and_then(|i| self.devices.get(i))
                    .is_some_and(|d| {
                        !self.dest_dir.trim().is_empty()
                            && is_same_volume(d, PathBuf::from(self.dest_dir.trim()).as_path())
                    });

                if same_vol {
                    ui.colored_label(RED, RichText::new(self.lang.same_drive_warning()).size(12.0));
                    ui.add_space(6.0);
                }

                if let Some(ref err) = self.recover_error {
                    ui.colored_label(RED, err);
                    ui.add_space(6.0);
                }

                if let Some(ref sum) = self.recover_summary {
                    ui.horizontal(|ui| {
                        let msg = if self.lang.is_tr() {
                            format!(
                                "Kurtarıldı: {} sağlam ({}) · {} sıfır bayt · {} TRIM ayrıldı · {} hata ({:.1} sn)",
                                sum.recovered_intact,
                                format_bytes(sum.bytes_written),
                                sum.recovered_zero_size,
                                sum.separated_trimmed,
                                sum.skipped_errors,
                                sum.elapsed_secs
                            )
                        } else {
                            format!(
                                "Recovered: {} intact ({}) · {} zero-byte · {} TRIM separated · {} errors ({:.1}s)",
                                sum.recovered_intact,
                                format_bytes(sum.bytes_written),
                                sum.recovered_zero_size,
                                sum.separated_trimmed,
                                sum.skipped_errors,
                                sum.elapsed_secs
                            )
                        };
                        ui.colored_label(EMERALD, RichText::new(msg).size(12.5).strong());
                        if subtle_button(ui, self.lang.btn_open_dest()).clicked() {
                            #[cfg(windows)]
                            let _ = std::process::Command::new("explorer")
                                .arg(&sum.dest_dir)
                                .spawn();
                        }
                    });
                    ui.add_space(6.0);
                }

                ui.horizontal(|ui| {
                    let sel_text = if self.lang.is_tr() {
                        format!(
                            "Seçili: {} dosya ({})",
                            self.selected_count,
                            format_bytes(self.selected_bytes)
                        )
                    } else {
                        format!(
                            "Selected: {} files ({})",
                            self.selected_count,
                            format_bytes(self.selected_bytes)
                        )
                    };
                    pill_badge(ui, &sel_text, TEXT_PRI, BG_SUBTLE, BORDER_SUBTLE);
                    ui.add_space(8.0);

                    ui.label(RichText::new(self.lang.dest_label()).size(12.5).color(TEXT_SEC));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.dest_dir)
                            .desired_width(250.0)
                            .margin(egui::Margin::symmetric(10, 6))
                            .hint_text("D:\\Restored"),
                    );
                    if subtle_button(ui, self.lang.btn_browse()).clicked() {
                        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                            self.dest_dir = folder.to_string_lossy().to_string();
                        }
                    }
                    ui.checkbox(&mut self.separate_trimmed, RichText::new(self.lang.separate_trim_opt()).size(12.0).color(TEXT_SEC));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.recovering {
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(self.lang.btn_stop_scan())
                                            .size(12.5)
                                            .strong()
                                            .color(RED),
                                    )
                                    .fill(RED_SOFT)
                                    .corner_radius(8.0),
                                )
                                .clicked()
                            {
                                self.recover_cancel.store(true, Ordering::Relaxed);
                            }
                            let frac = self.recover_done as f32 / self.recover_total.max(1) as f32;
                            ui.label(
                                RichText::new(format!("{}/{}", self.recover_done, self.recover_total))
                                    .size(12.0)
                                    .strong()
                                    .color(TEXT_PRI),
                            );
                            ui.add(
                                egui::ProgressBar::new(frac)
                                    .desired_width(150.0)
                                    .desired_height(8.0)
                                    .fill(ACCENT),
                            );
                        } else {
                            let can_recover = self.selected_count > 0
                                && !self.dest_dir.trim().is_empty()
                                && !same_vol;
                            let cta_label = if self.selected_count > 0 {
                                format!("{} ({}) →", self.lang.btn_recover(), self.selected_count)
                            } else {
                                format!("{} →", self.lang.btn_recover())
                            };
                            if primary_cta_button(
                                ui,
                                can_recover,
                                &cta_label,
                                ACCENT,
                                Vec2::new(150.0, 34.0),
                            )
                            .clicked()
                            {
                                self.start_recovery(ctx);
                            }
                        }
                    });
                });
            });
    }
}

fn short_device_title(dev: &DeviceInfo) -> String {
    if let Some(mp) = &dev.mount_point {
        let drive = mp.trim_end_matches('\\');
        if !dev.volume_label.is_empty() {
            return format!("{}  {}", drive, dev.volume_label);
        }
        if let (Some(l), Some(r)) = (dev.display_name.find('('), dev.display_name.find(')')) {
            if r > l + 1 {
                return format!("{}  {}", drive, &dev.display_name[l + 1..r]);
            }
        }
        return drive.to_string();
    }
    dev.display_name.clone()
}

fn section_header(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).size(12.0).strong().color(TEXT_SEC));
    ui.add_space(6.0);
}

fn section_header_with_right(
    ui: &mut egui::Ui,
    title: &str,
    right_ui: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(12.0).strong().color(TEXT_SEC));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right_ui);
    });
    ui.add_space(6.0);
}

fn pill_badge(ui: &mut egui::Ui, text: &str, fg: Color32, fill: Color32, border: Color32) {
    egui::Frame::NONE
        .fill(fill)
        .stroke(Stroke::new(1.0, border))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.0).strong().color(fg));
        });
}

fn pill_button(ui: &mut egui::Ui, active: bool, label: &str) -> egui::Response {
    let fill = if active { ACCENT_SOFT } else { Color32::TRANSPARENT };
    let stroke = if active {
        Stroke::new(1.0, ACCENT_BORDER)
    } else {
        Stroke::NONE
    };
    let fg = if active { ACCENT } else { TEXT_SEC };
    ui.add(
        egui::Button::new(RichText::new(label).size(12.0).strong().color(fg))
            .fill(fill)
            .stroke(stroke)
            .corner_radius(7.0),
    )
}

fn subtle_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).size(12.0).color(TEXT_PRI))
            .fill(BG_SUBTLE)
            .stroke(Stroke::new(1.0, BORDER_SUBTLE))
            .corner_radius(8.0),
    )
}

fn primary_cta_button(
    ui: &mut egui::Ui,
    enabled: bool,
    label: &str,
    fill: Color32,
    min_size: Vec2,
) -> egui::Response {
    let bg = if enabled { fill } else { BG_SUBTLE };
    let fg = if enabled { Color32::WHITE } else { TEXT_MUTED };
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(label).size(13.0).strong().color(fg))
            .fill(bg)
            .stroke(if enabled {
                Stroke::NONE
            } else {
                Stroke::new(1.0, BORDER_SUBTLE)
            })
            .corner_radius(9.0)
            .min_size(min_size),
    )
}

fn render_brand_mark(ui: &mut egui::Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 7.0, ACCENT);
    p.circle_stroke(
        rect.center(),
        size * 0.27,
        Stroke::new(2.0, Color32::WHITE),
    );
    p.circle_filled(rect.center(), size * 0.10, Color32::from_rgb(255, 237, 213));
}

fn health_color(h: FileHealth) -> Color32 {
    match h {
        FileHealth::Intact | FileHealth::Resident => EMERALD,
        FileHealth::ZeroByte => TEXT_MUTED,
        FileHealth::LikelyTrimmedZero => RED,
        FileHealth::PartialOrCorrupted => AMBER,
    }
}

fn format_hex_dump(bytes: &[u8]) -> String {
    let mut out = String::new();
    for (row, chunk) in bytes.chunks(8).enumerate() {
        out.push_str(&format!("{:04X}: ", row * 8));
        for b in chunk {
            out.push_str(&format!("{:02X} ", b));
        }
        for _ in chunk.len()..8 {
            out.push_str("   ");
        }
        out.push(' ');
        for &b in chunk {
            let c = if (0x20..=0x7E).contains(&b) {
                b as char
            } else {
                '.'
            };
            out.push(c);
        }
        out.push('\n');
    }
    out
}

fn load_system_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    #[cfg(windows)]
    {
        if let Ok(bytes) = std::fs::read("C:\\Windows\\Fonts\\segoeui.ttf") {
            fonts.font_data.insert(
                "segoe_ui".to_owned(),
                Arc::new(egui::FontData::from_owned(bytes)),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "segoe_ui".to_owned());
        }
        if let Ok(bytes) = std::fs::read("C:\\Windows\\Fonts\\seguisym.ttf") {
            fonts.font_data.insert(
                "segoe_sym".to_owned(),
                Arc::new(egui::FontData::from_owned(bytes)),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(1, "segoe_sym".to_owned());
        }
        if let Ok(bytes) = std::fs::read("C:\\Windows\\Fonts\\consola.ttf") {
            fonts.font_data.insert(
                "consolas".to_owned(),
                Arc::new(egui::FontData::from_owned(bytes)),
            );
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .insert(0, "consolas".to_owned());
        }
    }

    ctx.set_fonts(fonts);
}

fn apply_theme(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Light);
    ctx.style_mut_of(egui::Theme::Light, |style| {
        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
        style.spacing.button_padding = Vec2::new(11.0, 5.5);
        style.spacing.scroll.bar_width = 7.0;

        let v = &mut style.visuals;
        *v = egui::Visuals::light();
        v.panel_fill = BG_SURFACE;
        v.window_fill = BG_CANVAS;
        v.faint_bg_color = Color32::from_rgb(250, 249, 247);
        v.extreme_bg_color = BG_SUBTLE;
        v.selection.bg_fill = ACCENT_SOFT;
        v.selection.stroke = Stroke::new(1.0, ACCENT);

        let r = CornerRadius::same(7);
        v.widgets.noninteractive.bg_fill = BG_SURFACE;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER_SUBTLE);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_SEC);
        v.widgets.noninteractive.corner_radius = r;

        v.widgets.inactive.bg_fill = BG_SUBTLE;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER_SUBTLE);
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT_PRI);
        v.widgets.inactive.corner_radius = r;

        v.widgets.hovered.bg_fill = BG_CARD_HOVER;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
        v.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT_PRI);
        v.widgets.hovered.corner_radius = r;

        v.widgets.active.bg_fill = BG_CARD_HOVER;
        v.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
        v.widgets.active.fg_stroke = Stroke::new(1.0, TEXT_PRI);
        v.widgets.active.corner_radius = r;
    });
}
