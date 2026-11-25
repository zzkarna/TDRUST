use eframe::egui;

use crate::widget_api::{
    WidgetCatalogEntry, WidgetDescriptor, WidgetHostClass, WidgetHostConfig, WidgetHostMode,
    WidgetId, WidgetKind, WidgetLifecycle, WidgetRenderStatus, WidgetSnapshot, widget_catalog,
};

pub const SHELL_TOP_BAR_HEIGHT: f32 = 36.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellFocusOwner {
    Shell,
    CommandPalette,
    Widget(WidgetId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellCommandAction {
    SpawnWidget(WidgetDescriptor),
    MinimizeWidget(WidgetDescriptor),
    RestoreWidget(WidgetDescriptor),
    TearOutWidget(WidgetDescriptor),
    ReturnWidgetToShell(WidgetDescriptor),
    SaveWorkspaceLayout,
    RestoreWorkspaceLayout,
    ResetWorkspaceLayout,
    ApplyWorkspacePreset(WorkspacePresetKind),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShellTopBarResponse {
    pub action: Option<ShellCommandAction>,
    pub toggle_debug_latency_overlay: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkspaceLayoutRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl WorkspaceLayoutRect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    fn from_host(host: &WidgetHostConfig) -> Self {
        Self {
            x: host.default_pos.x,
            y: host.default_pos.y,
            width: host.default_size.x,
            height: host.default_size.y,
        }
    }

    fn pos(self) -> egui::Pos2 {
        egui::pos2(self.x, self.y)
    }

    fn size(self) -> egui::Vec2 {
        egui::vec2(self.width, self.height)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceLayoutWidget {
    pub id: WidgetId,
    pub kind: WidgetKind,
    pub lifecycle: WidgetLifecycle,
    pub host_mode: WidgetHostMode,
    pub rect: WorkspaceLayoutRect,
    pub z_order: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceLayout {
    pub next_widget_id: u64,
    pub next_z_order: u64,
    pub active_widget_id: Option<WidgetId>,
    pub widgets: Vec<WorkspaceLayoutWidget>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceLayoutRestoreInput {
    pub next_widget_id: u64,
    pub next_z_order: u64,
    pub active_widget_id: Option<WidgetId>,
    pub widgets: Vec<WorkspaceLayoutWidget>,
}

impl From<WorkspaceLayoutRestoreInput> for WorkspaceLayout {
    fn from(input: WorkspaceLayoutRestoreInput) -> Self {
        Self {
            next_widget_id: input.next_widget_id,
            next_z_order: input.next_z_order,
            active_widget_id: input.active_widget_id,
            widgets: input.widgets,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspacePresetKind {
    OneLiveChart,
    TwoLiveCharts,
    LiveChartWithSyncedTimeframes,
}

impl WorkspacePresetKind {
    pub const fn mnemonic(self) -> &'static str {
        match self {
            Self::OneLiveChart => "P1",
            Self::TwoLiveCharts => "P2",
            Self::LiveChartWithSyncedTimeframes => "PTF",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::OneLiveChart => "One Live Chart",
            Self::TwoLiveCharts => "Two Live Charts",
            Self::LiveChartWithSyncedTimeframes => "Live Chart + Synced Timeframes",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::OneLiveChart => "Replace workspace with one chart widget",
            Self::TwoLiveCharts => "Replace workspace with two chart widgets",
            Self::LiveChartWithSyncedTimeframes => {
                "Replace workspace with chart and timeframe overview"
            }
        }
    }

    fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::OneLiveChart => &["preset", "one chart", "single chart"],
            Self::TwoLiveCharts => &["preset", "two charts", "dual chart", "2 chart"],
            Self::LiveChartWithSyncedTimeframes => {
                &["preset", "timeframe", "synced", "overview", "tf"]
            }
        }
    }
}

pub const WORKSPACE_PRESETS: [WorkspacePresetKind; 3] = [
    WorkspacePresetKind::OneLiveChart,
    WorkspacePresetKind::TwoLiveCharts,
    WorkspacePresetKind::LiveChartWithSyncedTimeframes,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellActionCommand {
    MinimizeActiveWidget,
    TearOutActiveWidget,
    ReturnActiveWidgetToShell,
    RestoreWidget(WidgetId),
    SaveWorkspaceLayout,
    RestoreWorkspaceLayout,
    ResetWorkspaceLayout,
    ApplyWorkspacePreset(WorkspacePresetKind),
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct WorkspaceWidgetInstance {
    descriptor: WidgetDescriptor,
    lifecycle: WidgetLifecycle,
    host: WidgetHostConfig,
    host_class: Option<WidgetHostClass>,
    z_order: u64,
}

#[derive(Debug, Clone)]
pub struct WorkspaceShell {
    command_open: bool,
    command_input: String,
    focus_command_input: bool,
    focus_owner: ShellFocusOwner,
    next_widget_id: u64,
    next_z_order: u64,
    widgets: Vec<WorkspaceWidgetInstance>,
}

impl Default for WorkspaceShell {
    fn default() -> Self {
        Self {
            command_open: false,
            command_input: String::new(),
            focus_command_input: false,
            focus_owner: ShellFocusOwner::Shell,
            next_widget_id: 1,
            next_z_order: 1,
            widgets: Vec::new(),
        }
    }
}

impl WorkspaceShell {
    pub fn command_open(&self) -> bool {
        self.command_open
    }

    #[cfg(test)]
    pub fn has_open_widgets(&self) -> bool {
        self.widgets
            .iter()
            .any(|widget| widget.lifecycle == WidgetLifecycle::Open)
    }

    pub fn has_workspace_widgets(&self) -> bool {
        self.widgets
            .iter()
            .any(|widget| widget.lifecycle != WidgetLifecycle::Closed)
    }

    pub fn widget_snapshots(&self) -> Vec<WidgetSnapshot> {
        let mut snapshots: Vec<_> = self
            .widgets
            .iter()
            .filter(|widget| widget.lifecycle == WidgetLifecycle::Open)
            .map(|widget| WidgetSnapshot {
                descriptor: widget.descriptor,
                lifecycle: widget.lifecycle,
                host: widget.host,
                host_class: widget.host_class,
                z_order: widget.z_order,
            })
            .collect();
        snapshots.sort_by_key(|widget| widget.z_order);
        snapshots
    }

    pub fn workspace_layout_snapshot(&self) -> WorkspaceLayout {
        let mut widgets: Vec<_> = self
            .widgets
            .iter()
            .filter(|widget| widget.lifecycle != WidgetLifecycle::Closed)
            .map(|widget| WorkspaceLayoutWidget {
                id: widget.descriptor.id,
                kind: widget.descriptor.kind,
                lifecycle: widget.lifecycle,
                host_mode: widget.host.mode,
                rect: WorkspaceLayoutRect::from_host(&widget.host),
                z_order: widget.z_order,
            })
            .collect();
        widgets.sort_by_key(|widget| widget.z_order);
        let active_widget_id = self
            .active_widget_id()
            .filter(|id| widgets.iter().any(|widget| widget.id == *id));
        WorkspaceLayout {
            next_widget_id: self.next_widget_id,
            next_z_order: self.next_z_order,
            active_widget_id,
            widgets,
        }
    }

    pub fn set_widget_layout_rect(&mut self, id: WidgetId, rect: WorkspaceLayoutRect) -> bool {
        let Some(index) = self.widget_index(id) else {
            return false;
        };
        if self.widgets[index].lifecycle == WidgetLifecycle::Closed {
            return false;
        }

        let host = &mut self.widgets[index].host;
        let min_size = match host.mode {
            WidgetHostMode::InShellWorkspaceSurface => host.in_shell_min_size,
            WidgetHostMode::DetachedNativeViewport => host.min_size,
        };
        let pos = rect.pos();
        let size = egui::vec2(rect.width.max(min_size.x), rect.height.max(min_size.y));
        if !layout_f32_changed(host.default_pos.x, pos.x)
            && !layout_f32_changed(host.default_pos.y, pos.y)
            && !layout_f32_changed(host.default_size.x, size.x)
            && !layout_f32_changed(host.default_size.y, size.y)
        {
            return false;
        }

        host.default_pos = pos;
        host.default_size = size;
        true
    }

    pub fn restore_workspace_layout(&mut self, input: WorkspaceLayoutRestoreInput) {
        let mut max_widget_id = 0;
        let mut max_z_order = 0;
        let mut widgets: Vec<_> = input
            .widgets
            .into_iter()
            .filter(|widget| widget.lifecycle != WidgetLifecycle::Closed)
            .map(|widget| {
                max_widget_id = max_widget_id.max(widget.id.get());
                max_z_order = max_z_order.max(widget.z_order);
                let mut host = widget.kind.default_host_config(widget.rect.pos());
                host.mode = widget.host_mode;
                host.default_size = widget.rect.size();
                WorkspaceWidgetInstance {
                    descriptor: WidgetDescriptor::new(widget.id, widget.kind),
                    lifecycle: widget.lifecycle,
                    host,
                    host_class: None,
                    z_order: widget.z_order,
                }
            })
            .collect();
        widgets.sort_by_key(|widget| widget.z_order);
        self.next_widget_id = input
            .next_widget_id
            .max(max_widget_id.saturating_add(1))
            .max(1);
        self.next_z_order = input.next_z_order.max(max_z_order.saturating_add(1)).max(1);
        self.widgets = widgets;
        self.command_open = false;
        self.command_input.clear();
        self.focus_command_input = false;
        self.focus_owner = input
            .active_widget_id
            .filter(|id| {
                self.widgets.iter().any(|widget| {
                    widget.descriptor.id == *id && widget.lifecycle == WidgetLifecycle::Open
                })
            })
            .map(ShellFocusOwner::Widget)
            .unwrap_or(ShellFocusOwner::Shell);
    }

    pub fn reset_workspace_layout(&mut self) {
        self.command_open = false;
        self.command_input.clear();
        self.focus_command_input = false;
        self.focus_owner = ShellFocusOwner::Shell;
        self.next_widget_id = 1;
        self.next_z_order = 1;
        self.widgets.clear();
    }

    pub fn apply_workspace_preset(&mut self, preset: WorkspacePresetKind) -> Vec<WidgetDescriptor> {
        let widgets = workspace_preset_widgets(preset);
        let descriptors: Vec<_> = widgets
            .iter()
            .map(|widget| WidgetDescriptor::new(widget.id, widget.kind))
            .collect();
        let active_widget_id = widgets
            .iter()
            .rev()
            .find(|widget| widget.lifecycle == WidgetLifecycle::Open)
            .map(|widget| widget.id);
        self.restore_workspace_layout(WorkspaceLayoutRestoreInput {
            next_widget_id: widgets
                .iter()
                .map(|widget| widget.id.get())
                .max()
                .unwrap_or(0)
                .saturating_add(1)
                .max(1),
            next_z_order: widgets
                .iter()
                .map(|widget| widget.z_order)
                .max()
                .unwrap_or(0)
                .saturating_add(1)
                .max(1),
            active_widget_id,
            widgets,
        });
        descriptors
    }

    pub fn set_widget_render_status(&mut self, status: WidgetRenderStatus) -> bool {
        if let Some(index) = self.widget_index(status.id) {
            let mut layout_changed = false;
            if self.widgets[index].lifecycle != status.lifecycle {
                self.widgets[index].lifecycle = status.lifecycle;
                layout_changed = true;
            }
            self.widgets[index].host_class = Some(status.host_class);
            if status.focused && status.is_open() {
                layout_changed |= self.focus_widget(status.id);
            }
            if !status.is_open() && self.focus_owner == ShellFocusOwner::Widget(status.id) {
                self.focus_owner = ShellFocusOwner::Shell;
                layout_changed = true;
            }
            layout_changed
        } else {
            false
        }
    }

    pub fn has_detached_native_viewport(&self) -> bool {
        self.widgets.iter().any(|widget| {
            widget.lifecycle != WidgetLifecycle::Closed
                && (widget.host.mode == WidgetHostMode::DetachedNativeViewport
                    || widget.host_class == Some(WidgetHostClass::NativeViewport))
        })
    }

    pub fn active_widget_id(&self) -> Option<WidgetId> {
        match self.focus_owner {
            ShellFocusOwner::Widget(id)
                if self.widgets.iter().any(|widget| {
                    widget.lifecycle == WidgetLifecycle::Open && widget.descriptor.id == id
                }) =>
            {
                Some(id)
            }
            _ => self
                .widgets
                .iter()
                .filter(|widget| widget.lifecycle == WidgetLifecycle::Open)
                .max_by_key(|widget| widget.z_order)
                .map(|widget| widget.descriptor.id),
        }
    }

    #[cfg(test)]
    pub fn focus_owner(&self) -> ShellFocusOwner {
        self.focus_owner
    }

    pub fn handle_keyboard(&mut self, ctx: &egui::Context) -> Option<ShellCommandAction> {
        if !self.command_open
            && ctx.input(|input| {
                input.key_pressed(egui::Key::Backtick)
                    && !input.modifiers.command
                    && !input.modifiers.ctrl
                    && !input.modifiers.alt
            })
        {
            if !ctx.egui_wants_keyboard_input() {
                self.open_command();
            }
            return None;
        }

        if !self.command_open {
            return None;
        }

        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.close_command();
            return None;
        }

        if ctx.input(|input| input.key_pressed(egui::Key::Enter)) {
            return self.execute_first_matching_command();
        }

        None
    }

    pub fn show_top_bar(
        &mut self,
        ui: &mut egui::Ui,
        debug_latency_overlay_visible: bool,
    ) -> ShellTopBarResponse {
        let rect = ui.max_rect();
        let painter = ui.painter();
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(18, 18, 18));
        painter.line_segment(
            [
                egui::pos2(rect.left(), rect.bottom() - 1.0),
                egui::pos2(rect.right(), rect.bottom() - 1.0),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_rgb(45, 45, 45)),
        );

        let mut top_bar_response = ShellTopBarResponse::default();
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(rect.shrink2(egui::vec2(8.0, 4.0))),
            |ui| {
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.label(
                        egui::RichText::new(">")
                            .monospace()
                            .size(17.0)
                            .color(egui::Color32::from_rgb(226, 228, 232)),
                    );

                    if self.command_open {
                        let edit = egui::TextEdit::singleline(&mut self.command_input)
                            .id(egui::Id::new("workspace_shell_command_input"))
                            .font(egui::TextStyle::Monospace)
                            .hint_text("Type a command...")
                            .desired_width(360.0)
                            .frame(egui::Frame::NONE);
                        let response = ui.add(edit);
                        if self.focus_command_input {
                            response.request_focus();
                            self.focus_command_input = false;
                        }
                    } else {
                        let response = ui.add(
                            egui::Button::new(
                                egui::RichText::new("Backtick (`) to open widgets")
                                    .monospace()
                                    .size(13.0),
                            )
                            .frame(false),
                        );
                        if response.clicked() {
                            self.open_command();
                        }
                    }

                    ui.add_space(12.0);
                    let chart_count = self
                        .widget_snapshots()
                        .iter()
                        .filter(|widget| widget.kind() == WidgetKind::LiveChart)
                        .count();
                    let response = show_shell_mnemonic(ui, chart_count);
                    if response.clicked() {
                        top_bar_response.action = self.spawn_widget(WidgetKind::LiveChart);
                    }
                    if cfg!(debug_assertions)
                        && show_shell_toggle_button(
                            ui,
                            "DBG",
                            "Toggle debug latency panel",
                            debug_latency_overlay_visible,
                        )
                        .clicked()
                    {
                        top_bar_response.toggle_debug_latency_overlay = true;
                    }
                    if self.has_detached_native_viewport() {
                        ui.label(
                            egui::RichText::new(widget_host_label(
                                WidgetHostMode::DetachedNativeViewport,
                            ))
                            .monospace()
                            .size(12.0)
                            .color(egui::Color32::from_rgb(232, 178, 88)),
                        );
                    }

                    let minimized_widgets: Vec<_> = self
                        .widgets
                        .iter()
                        .filter(|widget| widget.lifecycle == WidgetLifecycle::Minimized)
                        .map(|widget| widget.descriptor)
                        .collect();
                    for descriptor in minimized_widgets {
                        let label = format!("R {}", descriptor.id);
                        let hover = format!("Restore {}", descriptor.title());
                        if show_shell_action_button(ui, &label, &hover).clicked() {
                            top_bar_response.action = self.restore_widget(descriptor.id);
                        }
                    }
                });
            },
        );

        top_bar_response
    }

    pub fn show_command_menu(&mut self, ctx: &egui::Context) -> Option<ShellCommandAction> {
        if !self.command_open {
            return None;
        }

        let mut action = None;
        egui::Area::new(egui::Id::new("workspace_shell_command_menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(0.0, SHELL_TOP_BAR_HEIGHT))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(17, 17, 17))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(55, 86, 72)))
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        ui.set_width(520.0);
                        let matching_actions = self.matching_shell_action_commands();
                        let matching_entries: Vec<_> = widget_catalog()
                            .iter()
                            .copied()
                            .filter(|entry| entry.matches_query(&self.command_input))
                            .collect();
                        if matching_actions.is_empty() && matching_entries.is_empty() {
                            ui.label(
                                egui::RichText::new("No matching commands")
                                    .monospace()
                                    .size(13.0)
                                    .color(egui::Color32::from_rgb(130, 134, 140)),
                            );
                        }

                        if !matching_actions.is_empty() {
                            ui.label(
                                egui::RichText::new("ACTIONS")
                                    .monospace()
                                    .size(12.0)
                                    .color(egui::Color32::from_rgb(170, 174, 180)),
                            );
                            ui.add_space(3.0);
                            for command in matching_actions {
                                let row = self.shell_action_row(command);
                                let row_response = show_shell_action_row(
                                    ui,
                                    &row.mnemonic,
                                    &row.label,
                                    &row.description,
                                );
                                if row_response.clicked() {
                                    action = self.execute_shell_action_command(command);
                                }
                            }
                            ui.add_space(6.0);
                        }

                        if !matching_entries.is_empty() {
                            ui.label(
                                egui::RichText::new("WIDGETS")
                                    .monospace()
                                    .size(12.0)
                                    .color(egui::Color32::from_rgb(170, 174, 180)),
                            );
                            ui.add_space(3.0);
                        }
                        for entry in matching_entries {
                            let row_response = show_command_row(ui, entry);
                            if row_response.clicked() {
                                action = self.spawn_widget(entry.kind);
                            }
                        }
                    });
            });

        action
    }

    pub fn show_workspace_background(&self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let painter = ui.painter();
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(13, 13, 13));
        painter.rect_stroke(
            rect.shrink(8.0),
            0.0,
            egui::Stroke::new(1.0, egui::Color32::from_rgb(34, 34, 34)),
            egui::StrokeKind::Inside,
        );
        if !self.has_workspace_widgets() {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Backtick (`) -> Live Chart",
                egui::FontId::monospace(16.0),
                egui::Color32::from_rgb(120, 124, 130),
            );
        }
    }

    fn open_command(&mut self) {
        self.command_open = true;
        self.command_input.clear();
        self.focus_command_input = true;
        self.focus_owner = ShellFocusOwner::CommandPalette;
    }

    fn close_command(&mut self) {
        self.command_open = false;
        self.command_input.clear();
        self.focus_command_input = false;
        self.focus_owner = ShellFocusOwner::Shell;
    }

    fn execute_first_matching_command(&mut self) -> Option<ShellCommandAction> {
        if !self.command_input.trim().is_empty()
            && let Some(command) = self.matching_shell_action_commands().into_iter().next()
        {
            return self.execute_shell_action_command(command);
        }
        self.execute_first_matching_widget_command()
    }

    fn execute_first_matching_widget_command(&mut self) -> Option<ShellCommandAction> {
        let entry = widget_catalog()
            .iter()
            .copied()
            .find(|entry| entry.matches_query(&self.command_input))?;
        self.spawn_widget(entry.kind)
    }

    pub fn spawn_widget(&mut self, kind: WidgetKind) -> Option<ShellCommandAction> {
        self.command_open = false;
        self.command_input.clear();
        self.focus_command_input = false;

        let id = WidgetId::new(self.next_widget_id);
        self.next_widget_id += 1;
        let offset = ((id.get() - 1) as f32 * 28.0) % 168.0;
        let descriptor = WidgetDescriptor::new(id, kind);
        let default_pos = if kind == WidgetKind::BookmapOrderFlow {
            egui::pos2(16.0, SHELL_TOP_BAR_HEIGHT + 8.0)
        } else {
            egui::pos2(64.0 + offset, SHELL_TOP_BAR_HEIGHT + 42.0 + offset)
        };
        let z_order = self.allocate_z_order();
        self.widgets.push(WorkspaceWidgetInstance {
            descriptor,
            lifecycle: WidgetLifecycle::Open,
            host: kind.default_host_config(default_pos),
            host_class: None,
            z_order,
        });
        self.focus_owner = ShellFocusOwner::Widget(id);

        Some(ShellCommandAction::SpawnWidget(descriptor))
    }

    fn widget_index(&self, id: WidgetId) -> Option<usize> {
        self.widgets
            .iter()
            .position(|widget| widget.descriptor.id == id)
    }

    fn active_open_widget(&self) -> Option<WorkspaceWidgetInstance> {
        self.active_widget_id().and_then(|id| {
            self.widgets.iter().copied().find(|widget| {
                widget.descriptor.id == id && widget.lifecycle == WidgetLifecycle::Open
            })
        })
    }

    fn allocate_z_order(&mut self) -> u64 {
        let z_order = self.next_z_order;
        self.next_z_order += 1;
        z_order
    }

    fn focus_widget(&mut self, id: WidgetId) -> bool {
        let Some(index) = self.widget_index(id) else {
            return false;
        };
        if self.widgets[index].lifecycle != WidgetLifecycle::Open {
            return false;
        }
        let already_front = self.widgets[index].z_order
            >= self
                .widgets
                .iter()
                .filter(|widget| widget.lifecycle == WidgetLifecycle::Open)
                .map(|widget| widget.z_order)
                .max()
                .unwrap_or(0);
        if already_front {
            self.focus_owner = ShellFocusOwner::Widget(id);
            return false;
        }
        self.widgets[index].z_order = self.allocate_z_order();
        self.focus_owner = ShellFocusOwner::Widget(id);
        true
    }

    pub fn minimize_widget(&mut self, id: WidgetId) -> Option<ShellCommandAction> {
        let index = self.widget_index(id)?;
        if self.widgets[index].lifecycle != WidgetLifecycle::Open {
            return None;
        }
        let descriptor = self.widgets[index].descriptor;
        self.widgets[index].lifecycle = WidgetLifecycle::Minimized;
        if self.focus_owner == ShellFocusOwner::Widget(id) {
            self.focus_owner = ShellFocusOwner::Shell;
        }
        Some(ShellCommandAction::MinimizeWidget(descriptor))
    }

    fn restore_widget(&mut self, id: WidgetId) -> Option<ShellCommandAction> {
        let index = self.widget_index(id)?;
        if self.widgets[index].lifecycle == WidgetLifecycle::Closed {
            return None;
        }
        let descriptor = self.widgets[index].descriptor;
        self.widgets[index].lifecycle = WidgetLifecycle::Open;
        self.focus_widget(id);
        Some(ShellCommandAction::RestoreWidget(descriptor))
    }

    pub fn tear_out_widget(&mut self, id: WidgetId) -> Option<ShellCommandAction> {
        let index = self.widget_index(id)?;
        if self.widgets[index].lifecycle == WidgetLifecycle::Closed {
            return None;
        }
        let descriptor = self.widgets[index].descriptor;
        self.widgets[index].host.mode = WidgetHostMode::DetachedNativeViewport;
        self.widgets[index].lifecycle = WidgetLifecycle::Open;
        self.focus_widget(id);
        Some(ShellCommandAction::TearOutWidget(descriptor))
    }

    pub fn return_widget_to_shell(&mut self, id: WidgetId) -> Option<ShellCommandAction> {
        let index = self.widget_index(id)?;
        if self.widgets[index].lifecycle == WidgetLifecycle::Closed {
            return None;
        }
        let descriptor = self.widgets[index].descriptor;
        self.widgets[index].host.mode = WidgetHostMode::InShellWorkspaceSurface;
        self.widgets[index].lifecycle = WidgetLifecycle::Open;
        self.focus_widget(id);
        Some(ShellCommandAction::ReturnWidgetToShell(descriptor))
    }

    fn matching_shell_action_commands(&self) -> Vec<ShellActionCommand> {
        let mut commands = Vec::new();
        if let Some(widget) = self.active_open_widget() {
            if shell_command_matches(
                &self.command_input,
                "MIN",
                "Minimize active widget",
                "Hide the active widget surface",
                &["min", "hide"],
            ) {
                commands.push(ShellActionCommand::MinimizeActiveWidget);
            }
            match widget.host.mode {
                WidgetHostMode::InShellWorkspaceSurface => {
                    if shell_command_matches(
                        &self.command_input,
                        "OUT",
                        "Tear out active widget",
                        "Move the active widget to a detached native viewport",
                        &["tear", "detach", "out", "window"],
                    ) {
                        commands.push(ShellActionCommand::TearOutActiveWidget);
                    }
                }
                WidgetHostMode::DetachedNativeViewport => {
                    if shell_command_matches(
                        &self.command_input,
                        "IN",
                        "Return active widget to shell",
                        "Move the active widget back into the shell workspace",
                        &["return", "shell", "dock", "in"],
                    ) {
                        commands.push(ShellActionCommand::ReturnActiveWidgetToShell);
                    }
                }
            }
        }

        for widget in self
            .widgets
            .iter()
            .filter(|widget| widget.lifecycle == WidgetLifecycle::Minimized)
        {
            let label = format!("Restore {}", widget.descriptor.title());
            if shell_command_matches(
                &self.command_input,
                "RST",
                &label,
                "Show the minimized widget surface",
                &["restore", "show"],
            ) {
                commands.push(ShellActionCommand::RestoreWidget(widget.descriptor.id));
            }
        }

        if shell_command_matches(
            &self.command_input,
            "SAVE",
            "Save workspace layout",
            "Persist the current widget layout",
            &["save", "persist", "layout"],
        ) {
            commands.push(ShellActionCommand::SaveWorkspaceLayout);
        }
        if shell_command_matches(
            &self.command_input,
            "LOAD",
            "Restore workspace layout",
            "Load the saved widget layout",
            &["restore", "load", "reopen", "layout"],
        ) {
            commands.push(ShellActionCommand::RestoreWorkspaceLayout);
        }
        if shell_command_matches(
            &self.command_input,
            "RESET",
            "Reset workspace layout",
            "Clear the saved and current widget layout",
            &["reset", "clear", "default", "layout"],
        ) {
            commands.push(ShellActionCommand::ResetWorkspaceLayout);
        }
        for preset in WORKSPACE_PRESETS {
            if shell_command_matches(
                &self.command_input,
                preset.mnemonic(),
                preset.label(),
                preset.description(),
                preset.aliases(),
            ) {
                commands.push(ShellActionCommand::ApplyWorkspacePreset(preset));
            }
        }

        commands
    }

    fn shell_action_row(&self, command: ShellActionCommand) -> ShellActionRow {
        match command {
            ShellActionCommand::MinimizeActiveWidget => ShellActionRow::new(
                "MIN",
                "Minimize active widget",
                "Hide the active widget surface",
            ),
            ShellActionCommand::TearOutActiveWidget => ShellActionRow::new(
                "OUT",
                "Tear out active widget",
                "Move the active widget to a detached native viewport",
            ),
            ShellActionCommand::ReturnActiveWidgetToShell => ShellActionRow::new(
                "IN",
                "Return active widget to shell",
                "Move the active widget back into the shell workspace",
            ),
            ShellActionCommand::RestoreWidget(id) => {
                let label = self
                    .widgets
                    .iter()
                    .find(|widget| widget.descriptor.id == id)
                    .map(|widget| format!("Restore {}", widget.descriptor.title()))
                    .unwrap_or_else(|| "Restore widget".to_string());
                ShellActionRow::new("RST", label, "Show the minimized widget surface")
            }
            ShellActionCommand::SaveWorkspaceLayout => ShellActionRow::new(
                "SAVE",
                "Save workspace layout",
                "Persist the current widget layout",
            ),
            ShellActionCommand::RestoreWorkspaceLayout => ShellActionRow::new(
                "LOAD",
                "Restore workspace layout",
                "Load the saved widget layout",
            ),
            ShellActionCommand::ResetWorkspaceLayout => ShellActionRow::new(
                "RESET",
                "Reset workspace layout",
                "Clear the saved and current widget layout",
            ),
            ShellActionCommand::ApplyWorkspacePreset(preset) => {
                ShellActionRow::new(preset.mnemonic(), preset.label(), preset.description())
            }
        }
    }

    fn execute_shell_action_command(
        &mut self,
        command: ShellActionCommand,
    ) -> Option<ShellCommandAction> {
        match command {
            ShellActionCommand::MinimizeActiveWidget => self
                .active_widget_id()
                .and_then(|id| self.minimize_widget(id)),
            ShellActionCommand::TearOutActiveWidget => self
                .active_widget_id()
                .and_then(|id| self.tear_out_widget(id)),
            ShellActionCommand::ReturnActiveWidgetToShell => self
                .active_widget_id()
                .and_then(|id| self.return_widget_to_shell(id)),
            ShellActionCommand::RestoreWidget(id) => self.restore_widget(id),
            ShellActionCommand::SaveWorkspaceLayout => {
                self.close_command();
                Some(ShellCommandAction::SaveWorkspaceLayout)
            }
            ShellActionCommand::RestoreWorkspaceLayout => {
                self.close_command();
                Some(ShellCommandAction::RestoreWorkspaceLayout)
            }
            ShellActionCommand::ResetWorkspaceLayout => {
                self.close_command();
                Some(ShellCommandAction::ResetWorkspaceLayout)
            }
            ShellActionCommand::ApplyWorkspacePreset(preset) => {
                self.close_command();
                Some(ShellCommandAction::ApplyWorkspacePreset(preset))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ShellActionRow {
    mnemonic: String,
    label: String,
    description: String,
}

impl ShellActionRow {
    fn new(
        mnemonic: impl Into<String>,
        label: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            mnemonic: mnemonic.into(),
            label: label.into(),
            description: description.into(),
        }
    }
}

fn show_shell_mnemonic(ui: &mut egui::Ui, chart_count: usize) -> egui::Response {
    let label = if chart_count == 0 {
        "CH".to_string()
    } else {
        format!("CH {chart_count}")
    };
    ui.add(
        egui::Button::new(
            egui::RichText::new(label)
                .monospace()
                .size(12.0)
                .color(egui::Color32::from_rgb(226, 184, 96)),
        )
        .fill(egui::Color32::from_rgb(45, 34, 18))
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(112, 78, 24)))
        .min_size(egui::vec2(36.0, 24.0)),
    )
    .on_hover_text("Launch Live Chart widget")
}

fn show_shell_action_button(ui: &mut egui::Ui, label: &str, hover: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            egui::RichText::new(label)
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(226, 228, 232)),
        )
        .fill(egui::Color32::from_rgb(30, 32, 34))
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(68, 72, 76)))
        .min_size(egui::vec2(32.0, 22.0)),
    )
    .on_hover_text(hover)
}

fn show_shell_toggle_button(
    ui: &mut egui::Ui,
    label: &str,
    hover: &str,
    selected: bool,
) -> egui::Response {
    let (fill, stroke, text) = if selected {
        (
            egui::Color32::from_rgb(28, 50, 40),
            egui::Color32::from_rgb(62, 135, 98),
            egui::Color32::from_rgb(130, 232, 174),
        )
    } else {
        (
            egui::Color32::from_rgb(30, 32, 34),
            egui::Color32::from_rgb(68, 72, 76),
            egui::Color32::from_rgb(226, 228, 232),
        )
    };
    ui.add(
        egui::Button::new(
            egui::RichText::new(label)
                .monospace()
                .size(11.0)
                .color(text),
        )
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, stroke))
        .min_size(egui::vec2(34.0, 22.0)),
    )
    .on_hover_text(hover)
}

fn show_shell_action_row(
    ui: &mut egui::Ui,
    mnemonic: &str,
    label: &str,
    description: &str,
) -> egui::Response {
    let desired = egui::vec2(ui.available_width(), 30.0);
    let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click());
    let painter = ui.painter();
    let fill = if response.hovered() {
        egui::Color32::from_rgb(48, 48, 48)
    } else {
        egui::Color32::from_rgb(28, 28, 28)
    };
    painter.rect_filled(rect, 0.0, fill);

    let badge_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left() + 2.0, rect.top() + 4.0),
        egui::vec2(44.0, 22.0),
    );
    painter.rect_filled(badge_rect, 3.0, egui::Color32::from_rgb(24, 42, 38));
    painter.rect_stroke(
        badge_rect,
        3.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(55, 106, 90)),
        egui::StrokeKind::Inside,
    );
    painter.text(
        badge_rect.center(),
        egui::Align2::CENTER_CENTER,
        mnemonic,
        egui::FontId::monospace(12.0),
        egui::Color32::from_rgb(130, 218, 182),
    );
    painter.text(
        egui::pos2(rect.left() + 56.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::monospace(14.0),
        egui::Color32::from_rgb(230, 232, 235),
    );
    painter.text(
        egui::pos2(rect.left() + 250.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        description,
        egui::FontId::monospace(13.0),
        egui::Color32::from_rgb(160, 164, 170),
    );

    response
}

fn show_command_row(ui: &mut egui::Ui, command: WidgetCatalogEntry) -> egui::Response {
    let desired = egui::vec2(ui.available_width(), 30.0);
    let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click());
    let painter = ui.painter();
    let fill = if response.hovered() {
        egui::Color32::from_rgb(48, 48, 48)
    } else {
        egui::Color32::from_rgb(28, 28, 28)
    };
    painter.rect_filled(rect, 0.0, fill);

    let badge_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left() + 2.0, rect.top() + 4.0),
        egui::vec2(44.0, 22.0),
    );
    painter.rect_filled(badge_rect, 3.0, egui::Color32::from_rgb(54, 36, 14));
    painter.rect_stroke(
        badge_rect,
        3.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(116, 78, 24)),
        egui::StrokeKind::Inside,
    );
    painter.text(
        badge_rect.center(),
        egui::Align2::CENTER_CENTER,
        command.kind.mnemonic(),
        egui::FontId::monospace(12.0),
        egui::Color32::from_rgb(232, 178, 88),
    );
    painter.text(
        egui::pos2(rect.left() + 56.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        command.kind.label(),
        egui::FontId::monospace(14.0),
        egui::Color32::from_rgb(230, 232, 235),
    );
    painter.text(
        egui::pos2(rect.left() + 210.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        command.kind.description(),
        egui::FontId::monospace(13.0),
        egui::Color32::from_rgb(160, 164, 170),
    );

    painter.text(
        egui::pos2(rect.right() - 88.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        command.category.label(),
        egui::FontId::monospace(12.0),
        egui::Color32::from_rgb(118, 122, 128),
    );

    response
}

fn widget_host_label(host_mode: WidgetHostMode) -> &'static str {
    host_mode.label()
}

fn layout_f32_changed(previous: f32, next: f32) -> bool {
    (previous - next).abs() > 0.5
}

fn workspace_preset_widgets(preset: WorkspacePresetKind) -> Vec<WorkspaceLayoutWidget> {
    match preset {
        WorkspacePresetKind::OneLiveChart => vec![WorkspaceLayoutWidget {
            id: WidgetId::new(1),
            kind: WidgetKind::LiveChart,
            lifecycle: WidgetLifecycle::Open,
            host_mode: WidgetHostMode::InShellWorkspaceSurface,
            rect: WorkspaceLayoutRect::new(64.0, SHELL_TOP_BAR_HEIGHT + 42.0, 760.0, 440.0),
            z_order: 1,
        }],
        WorkspacePresetKind::TwoLiveCharts => vec![
            WorkspaceLayoutWidget {
                id: WidgetId::new(1),
                kind: WidgetKind::LiveChart,
                lifecycle: WidgetLifecycle::Open,
                host_mode: WidgetHostMode::InShellWorkspaceSurface,
                rect: WorkspaceLayoutRect::new(64.0, SHELL_TOP_BAR_HEIGHT + 42.0, 680.0, 430.0),
                z_order: 1,
            },
            WorkspaceLayoutWidget {
                id: WidgetId::new(2),
                kind: WidgetKind::LiveChart,
                lifecycle: WidgetLifecycle::Open,
                host_mode: WidgetHostMode::InShellWorkspaceSurface,
                rect: WorkspaceLayoutRect::new(772.0, SHELL_TOP_BAR_HEIGHT + 42.0, 680.0, 430.0),
                z_order: 2,
            },
        ],
        WorkspacePresetKind::LiveChartWithSyncedTimeframes => vec![
            WorkspaceLayoutWidget {
                id: WidgetId::new(1),
                kind: WidgetKind::LiveChart,
                lifecycle: WidgetLifecycle::Open,
                host_mode: WidgetHostMode::InShellWorkspaceSurface,
                rect: WorkspaceLayoutRect::new(64.0, SHELL_TOP_BAR_HEIGHT + 42.0, 760.0, 440.0),
                z_order: 1,
            },
            WorkspaceLayoutWidget {
                id: WidgetId::new(2),
                kind: WidgetKind::SyncedTimeframes,
                lifecycle: WidgetLifecycle::Open,
                host_mode: WidgetHostMode::InShellWorkspaceSurface,
                rect: WorkspaceLayoutRect::new(852.0, SHELL_TOP_BAR_HEIGHT + 42.0, 620.0, 440.0),
                z_order: 2,
            },
        ],
    }
}

fn shell_command_matches(
    input: &str,
    mnemonic: &str,
    label: &str,
    description: &str,
    aliases: &[&str],
) -> bool {
    let query = input.trim().to_ascii_lowercase();
    !query.is_empty()
        && (mnemonic.to_ascii_lowercase().contains(&query)
            || label.to_ascii_lowercase().contains(&query)
            || description.to_ascii_lowercase().contains(&query)
            || aliases.iter().any(|alias| alias.contains(&query)))
}

#[cfg(test)]
mod tests {
    use super::{
        SHELL_TOP_BAR_HEIGHT, ShellCommandAction, ShellFocusOwner, WorkspaceLayoutRect,
        WorkspaceLayoutRestoreInput, WorkspaceLayoutWidget, WorkspacePresetKind, WorkspaceShell,
    };
    use crate::widget_api::{
        WidgetDescriptor, WidgetHostClass, WidgetHostMode, WidgetId, WidgetKind, WidgetLifecycle,
        WidgetRenderStatus,
    };
    use eframe::egui;

    #[test]
    fn shell_starts_without_chart_widgets() {
        let shell = WorkspaceShell::default();
        assert_eq!(shell.focus_owner(), ShellFocusOwner::Shell);
        assert!(!shell.command_open());
        assert!(!shell.has_open_widgets());
        assert!(shell.widget_snapshots().is_empty());
    }

    #[test]
    fn spawning_live_chart_creates_a_widget() {
        let mut shell = WorkspaceShell::default();
        shell.open_command();
        let action = shell.spawn_widget(WidgetKind::LiveChart);
        assert_eq!(
            action,
            Some(ShellCommandAction::SpawnWidget(WidgetDescriptor::new(
                WidgetId::new(1),
                WidgetKind::LiveChart
            )))
        );
        let widgets = shell.widget_snapshots();
        assert_eq!(widgets.len(), 1);
        assert_eq!(
            widgets[0].host.mode,
            WidgetHostMode::InShellWorkspaceSurface
        );
        assert_eq!(widgets[0].host_class, None);
        assert_eq!(
            shell.focus_owner(),
            ShellFocusOwner::Widget(WidgetId::new(1))
        );
        assert!(!shell.command_open());
    }

    #[test]
    fn spawning_bookmap_order_flow_uses_maximized_workspace_defaults() {
        let mut shell = WorkspaceShell::default();
        shell.open_command();
        let action = shell.spawn_widget(WidgetKind::BookmapOrderFlow);
        assert_eq!(
            action,
            Some(ShellCommandAction::SpawnWidget(WidgetDescriptor::new(
                WidgetId::new(1),
                WidgetKind::BookmapOrderFlow
            )))
        );

        let widgets = shell.widget_snapshots();
        assert_eq!(widgets.len(), 1);
        assert_eq!(
            widgets[0].host.default_pos,
            egui::pos2(16.0, SHELL_TOP_BAR_HEIGHT + 8.0)
        );
        assert_eq!(widgets[0].host.default_size, egui::vec2(1_840.0, 1_020.0));
        assert_eq!(widgets[0].host.in_shell_min_size, egui::vec2(960.0, 640.0));
    }

    #[test]
    fn live_chart_command_can_spawn_multiple_widgets() {
        let mut shell = WorkspaceShell::default();
        assert_eq!(
            shell.spawn_widget(WidgetKind::LiveChart),
            Some(ShellCommandAction::SpawnWidget(WidgetDescriptor::new(
                WidgetId::new(1),
                WidgetKind::LiveChart
            )))
        );
        assert_eq!(
            shell.spawn_widget(WidgetKind::LiveChart),
            Some(ShellCommandAction::SpawnWidget(WidgetDescriptor::new(
                WidgetId::new(2),
                WidgetKind::LiveChart
            )))
        );
        let widgets = shell.widget_snapshots();
        assert_eq!(widgets.len(), 2);
        assert_eq!(widgets[0].id(), WidgetId::new(1));
        assert_eq!(widgets[1].id(), WidgetId::new(2));
    }

    #[test]
    fn widget_viewport_ids_are_stable_and_distinct() {
        let descriptor = WidgetDescriptor::new(WidgetId::new(7), WidgetKind::LiveChart);
        assert_eq!(
            descriptor.viewport_id(),
            WidgetDescriptor::new(WidgetId::new(7), WidgetKind::LiveChart).viewport_id()
        );
        assert_ne!(
            descriptor.viewport_id(),
            WidgetDescriptor::new(WidgetId::new(8), WidgetKind::LiveChart).viewport_id()
        );
    }

    #[test]
    fn in_shell_and_detached_host_modes_remain_distinct() {
        assert_ne!(
            WidgetHostMode::InShellWorkspaceSurface,
            WidgetHostMode::DetachedNativeViewport
        );
    }

    #[test]
    fn render_status_tracks_host_class_focus_and_close() {
        let mut shell = WorkspaceShell::default();
        assert_eq!(
            shell.spawn_widget(WidgetKind::LiveChart),
            Some(ShellCommandAction::SpawnWidget(WidgetDescriptor::new(
                WidgetId::new(1),
                WidgetKind::LiveChart
            )))
        );

        shell.set_widget_render_status(WidgetRenderStatus::open(
            WidgetId::new(1),
            WidgetHostClass::NativeViewport,
            true,
        ));
        let widgets = shell.widget_snapshots();
        assert_eq!(widgets[0].host_class, Some(WidgetHostClass::NativeViewport));
        assert_eq!(
            shell.focus_owner(),
            ShellFocusOwner::Widget(WidgetId::new(1))
        );

        shell.set_widget_render_status(WidgetRenderStatus::closed(
            WidgetId::new(1),
            WidgetHostClass::NativeViewport,
        ));
        assert!(!shell.has_open_widgets());
        assert_eq!(shell.focus_owner(), ShellFocusOwner::Shell);
    }

    #[test]
    fn closed_widget_snapshots_are_not_returned() {
        let mut shell = WorkspaceShell::default();
        assert_eq!(
            shell.spawn_widget(WidgetKind::LiveChart),
            Some(ShellCommandAction::SpawnWidget(WidgetDescriptor::new(
                WidgetId::new(1),
                WidgetKind::LiveChart
            )))
        );

        shell.set_widget_render_status(WidgetRenderStatus::closed(
            WidgetId::new(1),
            WidgetHostClass::NativeViewport,
        ));
        assert!(shell.widget_snapshots().is_empty());
    }

    #[test]
    fn detached_native_viewport_host_class_is_reported() {
        let mut shell = WorkspaceShell::default();
        assert_eq!(
            shell.spawn_widget(WidgetKind::LiveChart),
            Some(ShellCommandAction::SpawnWidget(WidgetDescriptor::new(
                WidgetId::new(1),
                WidgetKind::LiveChart
            )))
        );
        assert!(!shell.has_detached_native_viewport());

        shell.set_widget_render_status(WidgetRenderStatus::open(
            WidgetId::new(1),
            WidgetHostClass::NativeViewport,
            false,
        ));
        assert!(shell.has_detached_native_viewport());
    }

    #[test]
    fn active_widget_prefers_focused_open_widget() {
        let mut shell = WorkspaceShell::default();
        assert_eq!(
            shell.spawn_widget(WidgetKind::LiveChart),
            Some(ShellCommandAction::SpawnWidget(WidgetDescriptor::new(
                WidgetId::new(1),
                WidgetKind::LiveChart
            )))
        );
        assert_eq!(
            shell.spawn_widget(WidgetKind::LiveChart),
            Some(ShellCommandAction::SpawnWidget(WidgetDescriptor::new(
                WidgetId::new(2),
                WidgetKind::LiveChart
            )))
        );

        shell.set_widget_render_status(WidgetRenderStatus::open(
            WidgetId::new(1),
            WidgetHostClass::NativeViewport,
            true,
        ));
        assert_eq!(shell.active_widget_id(), Some(WidgetId::new(1)));
        shell.set_widget_render_status(WidgetRenderStatus::open(
            WidgetId::new(2),
            WidgetHostClass::NativeViewport,
            true,
        ));
        assert_eq!(shell.active_widget_id(), Some(WidgetId::new(2)));
        shell.set_widget_render_status(WidgetRenderStatus::closed(
            WidgetId::new(2),
            WidgetHostClass::NativeViewport,
        ));
        assert_eq!(shell.active_widget_id(), Some(WidgetId::new(1)));
    }

    #[test]
    fn focused_widget_moves_to_top_of_shell_z_order() {
        let mut shell = WorkspaceShell::default();
        shell.spawn_widget(WidgetKind::LiveChart);
        shell.spawn_widget(WidgetKind::Watchlist);

        let widgets = shell.widget_snapshots();
        assert_eq!(widgets[0].id(), WidgetId::new(1));
        assert_eq!(widgets[1].id(), WidgetId::new(2));
        assert!(widgets[0].z_order < widgets[1].z_order);

        shell.set_widget_render_status(WidgetRenderStatus::open(
            WidgetId::new(1),
            WidgetHostClass::InShellWorkspaceSurface,
            true,
        ));

        let widgets = shell.widget_snapshots();
        assert_eq!(widgets[0].id(), WidgetId::new(2));
        assert_eq!(widgets[1].id(), WidgetId::new(1));
        assert!(widgets[0].z_order < widgets[1].z_order);
        assert_eq!(shell.active_widget_id(), Some(WidgetId::new(1)));
    }

    #[test]
    fn focused_widget_does_not_churn_z_order_when_already_front() {
        let mut shell = WorkspaceShell::default();
        shell.spawn_widget(WidgetKind::LiveChart);
        shell.spawn_widget(WidgetKind::SyncedTimeframes);

        assert!(shell.set_widget_render_status(WidgetRenderStatus::open(
            WidgetId::new(1),
            WidgetHostClass::InShellWorkspaceSurface,
            true,
        )));
        let focused = shell.workspace_layout_snapshot();

        assert!(!shell.set_widget_render_status(WidgetRenderStatus::open(
            WidgetId::new(1),
            WidgetHostClass::InShellWorkspaceSurface,
            true,
        )));
        assert_eq!(shell.workspace_layout_snapshot(), focused);
    }

    #[test]
    fn front_widget_focus_does_not_allocate_z_order_after_shell_focus() {
        let mut shell = WorkspaceShell::default();
        shell.restore_workspace_layout(WorkspaceLayoutRestoreInput {
            next_widget_id: 6,
            next_z_order: 6,
            active_widget_id: None,
            widgets: vec![
                WorkspaceLayoutWidget {
                    id: WidgetId::new(1),
                    kind: WidgetKind::LiveChart,
                    lifecycle: WidgetLifecycle::Open,
                    host_mode: WidgetHostMode::InShellWorkspaceSurface,
                    rect: WorkspaceLayoutRect::new(56.0, 86.0, 620.0, 450.0),
                    z_order: 1,
                },
                WorkspaceLayoutWidget {
                    id: WidgetId::new(5),
                    kind: WidgetKind::LiveChart,
                    lifecycle: WidgetLifecycle::Open,
                    host_mode: WidgetHostMode::DetachedNativeViewport,
                    rect: WorkspaceLayoutRect::new(120.0, 120.0, 820.0, 520.0),
                    z_order: 5,
                },
            ],
        });
        assert_eq!(shell.focus_owner(), ShellFocusOwner::Shell);
        let focused = shell.workspace_layout_snapshot();

        assert!(!shell.set_widget_render_status(WidgetRenderStatus::open(
            WidgetId::new(5),
            WidgetHostClass::NativeViewport,
            true,
        )));

        assert_eq!(
            shell.focus_owner(),
            ShellFocusOwner::Widget(WidgetId::new(5))
        );
        assert_eq!(shell.workspace_layout_snapshot(), focused);
    }

    #[test]
    fn minimized_widgets_leave_render_order_but_not_workspace_ownership() {
        let mut shell = WorkspaceShell::default();
        shell.spawn_widget(WidgetKind::LiveChart);

        assert_eq!(
            shell.minimize_widget(WidgetId::new(1)),
            Some(ShellCommandAction::MinimizeWidget(WidgetDescriptor::new(
                WidgetId::new(1),
                WidgetKind::LiveChart
            )))
        );
        assert!(!shell.has_open_widgets());
        assert!(shell.has_workspace_widgets());
        assert!(shell.widget_snapshots().is_empty());
        assert_eq!(shell.active_widget_id(), None);
        assert_eq!(shell.focus_owner(), ShellFocusOwner::Shell);
    }

    #[test]
    fn restoring_minimized_widget_reopens_and_focuses_it() {
        let mut shell = WorkspaceShell::default();
        shell.spawn_widget(WidgetKind::LiveChart);
        shell.minimize_widget(WidgetId::new(1));

        assert_eq!(
            shell.restore_widget(WidgetId::new(1)),
            Some(ShellCommandAction::RestoreWidget(WidgetDescriptor::new(
                WidgetId::new(1),
                WidgetKind::LiveChart
            )))
        );

        let widgets = shell.widget_snapshots();
        assert_eq!(widgets.len(), 1);
        assert_eq!(widgets[0].lifecycle, WidgetLifecycle::Open);
        assert_eq!(shell.active_widget_id(), Some(WidgetId::new(1)));
        assert_eq!(
            shell.focus_owner(),
            ShellFocusOwner::Widget(WidgetId::new(1))
        );
    }

    #[test]
    fn tear_out_and_return_update_host_mode_without_replacing_widget() {
        let mut shell = WorkspaceShell::default();
        shell.spawn_widget(WidgetKind::LiveChart);

        assert_eq!(
            shell.tear_out_widget(WidgetId::new(1)),
            Some(ShellCommandAction::TearOutWidget(WidgetDescriptor::new(
                WidgetId::new(1),
                WidgetKind::LiveChart
            )))
        );
        let widgets = shell.widget_snapshots();
        assert_eq!(widgets[0].id(), WidgetId::new(1));
        assert_eq!(widgets[0].host.mode, WidgetHostMode::DetachedNativeViewport);
        assert!(shell.has_detached_native_viewport());

        assert_eq!(
            shell.return_widget_to_shell(WidgetId::new(1)),
            Some(ShellCommandAction::ReturnWidgetToShell(
                WidgetDescriptor::new(WidgetId::new(1), WidgetKind::LiveChart)
            ))
        );
        let widgets = shell.widget_snapshots();
        assert_eq!(widgets[0].id(), WidgetId::new(1));
        assert_eq!(
            widgets[0].host.mode,
            WidgetHostMode::InShellWorkspaceSurface
        );
    }

    #[test]
    fn command_query_executes_shell_host_actions() {
        let mut shell = WorkspaceShell::default();
        shell.spawn_widget(WidgetKind::LiveChart);

        shell.command_input = "tear".to_string();
        assert_eq!(
            shell.execute_first_matching_command(),
            Some(ShellCommandAction::TearOutWidget(WidgetDescriptor::new(
                WidgetId::new(1),
                WidgetKind::LiveChart
            )))
        );
        assert_eq!(
            shell.widget_snapshots()[0].host.mode,
            WidgetHostMode::DetachedNativeViewport
        );

        shell.command_input = "return".to_string();
        assert_eq!(
            shell.execute_first_matching_command(),
            Some(ShellCommandAction::ReturnWidgetToShell(
                WidgetDescriptor::new(WidgetId::new(1), WidgetKind::LiveChart)
            ))
        );
        assert_eq!(
            shell.widget_snapshots()[0].host.mode,
            WidgetHostMode::InShellWorkspaceSurface
        );

        shell.command_input = "min".to_string();
        assert_eq!(
            shell.execute_first_matching_command(),
            Some(ShellCommandAction::MinimizeWidget(WidgetDescriptor::new(
                WidgetId::new(1),
                WidgetKind::LiveChart
            )))
        );

        shell.command_input = "restore".to_string();
        assert_eq!(
            shell.execute_first_matching_command(),
            Some(ShellCommandAction::RestoreWidget(WidgetDescriptor::new(
                WidgetId::new(1),
                WidgetKind::LiveChart
            )))
        );
    }

    #[test]
    fn command_query_executes_workspace_layout_actions() {
        let mut shell = WorkspaceShell::default();

        shell.command_input = "save workspace".to_string();
        assert_eq!(
            shell.execute_first_matching_command(),
            Some(ShellCommandAction::SaveWorkspaceLayout)
        );

        shell.command_input = "restore workspace".to_string();
        assert_eq!(
            shell.execute_first_matching_command(),
            Some(ShellCommandAction::RestoreWorkspaceLayout)
        );

        shell.command_input = "reset workspace".to_string();
        assert_eq!(
            shell.execute_first_matching_command(),
            Some(ShellCommandAction::ResetWorkspaceLayout)
        );
    }

    #[test]
    fn command_query_returns_workspace_preset_actions() {
        let mut shell = WorkspaceShell::default();

        shell.command_input = "two charts".to_string();
        assert_eq!(
            shell.execute_first_matching_command(),
            Some(ShellCommandAction::ApplyWorkspacePreset(
                WorkspacePresetKind::TwoLiveCharts
            ))
        );

        shell.command_input = "synced".to_string();
        assert_eq!(
            shell.execute_first_matching_command(),
            Some(ShellCommandAction::ApplyWorkspacePreset(
                WorkspacePresetKind::LiveChartWithSyncedTimeframes
            ))
        );
    }

    #[test]
    fn empty_command_query_with_active_widget_spawns_first_widget_not_host_action() {
        let mut shell = WorkspaceShell::default();
        shell.spawn_widget(WidgetKind::LiveChart);
        shell.command_input.clear();

        assert!(shell.matching_shell_action_commands().is_empty());
        assert_eq!(
            shell.execute_first_matching_command(),
            Some(ShellCommandAction::SpawnWidget(WidgetDescriptor::new(
                WidgetId::new(2),
                WidgetKind::LiveChart
            )))
        );

        let widgets = shell.widget_snapshots();
        assert_eq!(widgets.len(), 2);
        assert_eq!(widgets[0].lifecycle, WidgetLifecycle::Open);
        assert_eq!(widgets[1].lifecycle, WidgetLifecycle::Open);
    }

    #[test]
    fn empty_command_query_with_minimized_widget_spawns_first_widget_not_restore_action() {
        let mut shell = WorkspaceShell::default();
        shell.spawn_widget(WidgetKind::LiveChart);
        shell.minimize_widget(WidgetId::new(1));
        shell.command_input.clear();

        assert!(shell.matching_shell_action_commands().is_empty());
        assert_eq!(
            shell.execute_first_matching_command(),
            Some(ShellCommandAction::SpawnWidget(WidgetDescriptor::new(
                WidgetId::new(2),
                WidgetKind::LiveChart
            )))
        );

        let widgets = shell.widget_snapshots();
        assert_eq!(widgets.len(), 1);
        assert_eq!(widgets[0].id(), WidgetId::new(2));
        assert_eq!(widgets[0].lifecycle, WidgetLifecycle::Open);
    }

    #[test]
    fn command_query_spawns_the_first_matching_widget_kind() {
        let mut shell = WorkspaceShell::default();
        shell.command_input = "news".to_string();
        assert_eq!(
            shell.execute_first_matching_widget_command(),
            Some(ShellCommandAction::SpawnWidget(WidgetDescriptor::new(
                WidgetId::new(1),
                WidgetKind::News
            )))
        );

        shell.open_command();
        shell.command_input = "broker".to_string();
        assert_eq!(shell.execute_first_matching_widget_command(), None);
    }

    #[test]
    fn widget_snapshots_preserve_lifecycle_and_descriptor() {
        let mut shell = WorkspaceShell::default();
        shell.spawn_widget(WidgetKind::LiveChart);
        let widget = shell.widget_snapshots()[0];
        assert_eq!(widget.lifecycle, WidgetLifecycle::Open);
        assert_eq!(widget.descriptor.kind, WidgetKind::LiveChart);
        assert_eq!(widget.title(), "Live Chart 1");
    }

    #[test]
    fn placeholder_widget_kinds_use_the_same_lifecycle_path() {
        let mut shell = WorkspaceShell::default();
        assert_eq!(
            shell.spawn_widget(WidgetKind::Watchlist),
            Some(ShellCommandAction::SpawnWidget(WidgetDescriptor::new(
                WidgetId::new(1),
                WidgetKind::Watchlist
            )))
        );
        let widget = shell.widget_snapshots()[0];
        assert_eq!(widget.kind(), WidgetKind::Watchlist);
        assert_eq!(widget.lifecycle, WidgetLifecycle::Open);
        assert_eq!(shell.active_widget_id(), Some(WidgetId::new(1)));
    }

    #[test]
    fn workspace_layout_snapshot_keeps_durable_widget_state() {
        let mut shell = WorkspaceShell::default();
        shell.spawn_widget(WidgetKind::LiveChart);
        shell.spawn_widget(WidgetKind::SyncedTimeframes);
        shell.minimize_widget(WidgetId::new(1));
        shell.tear_out_widget(WidgetId::new(2));

        let layout = shell.workspace_layout_snapshot();
        assert_eq!(layout.next_widget_id, 3);
        assert!(layout.next_z_order > 2);
        assert_eq!(layout.active_widget_id, Some(WidgetId::new(2)));
        assert_eq!(layout.widgets.len(), 2);

        let first = &layout.widgets[0];
        assert_eq!(first.id, WidgetId::new(1));
        assert_eq!(first.kind, WidgetKind::LiveChart);
        assert_eq!(first.lifecycle, WidgetLifecycle::Minimized);
        assert_eq!(first.host_mode, WidgetHostMode::InShellWorkspaceSurface);
        assert_eq!(first.rect.x, 64.0);
        assert_eq!(first.rect.y, 78.0);
        assert_eq!(first.rect.width, 760.0);
        assert_eq!(first.rect.height, 440.0);

        let second = &layout.widgets[1];
        assert_eq!(second.id, WidgetId::new(2));
        assert_eq!(second.kind, WidgetKind::SyncedTimeframes);
        assert_eq!(second.lifecycle, WidgetLifecycle::Open);
        assert_eq!(second.host_mode, WidgetHostMode::DetachedNativeViewport);
        assert!(first.z_order < second.z_order);
    }

    #[test]
    fn workspace_layout_snapshot_excludes_closed_widgets() {
        let mut shell = WorkspaceShell::default();
        shell.spawn_widget(WidgetKind::LiveChart);
        shell.set_widget_render_status(WidgetRenderStatus::closed(
            WidgetId::new(1),
            WidgetHostClass::InShellWorkspaceSurface,
        ));

        let layout = shell.workspace_layout_snapshot();
        assert!(layout.widgets.is_empty());
        assert_eq!(layout.active_widget_id, None);
        assert_eq!(layout.next_widget_id, 2);
    }

    #[test]
    fn workspace_layout_restore_rebuilds_shell_lifecycle_and_order() {
        let mut shell = WorkspaceShell::default();
        shell.restore_workspace_layout(WorkspaceLayoutRestoreInput {
            next_widget_id: 2,
            next_z_order: 3,
            active_widget_id: Some(WidgetId::new(5)),
            widgets: vec![
                WorkspaceLayoutWidget {
                    id: WidgetId::new(4),
                    kind: WidgetKind::LiveChart,
                    lifecycle: WidgetLifecycle::Minimized,
                    host_mode: WidgetHostMode::InShellWorkspaceSurface,
                    rect: WorkspaceLayoutRect::new(20.0, 40.0, 700.0, 420.0),
                    z_order: 10,
                },
                WorkspaceLayoutWidget {
                    id: WidgetId::new(5),
                    kind: WidgetKind::SyncedTimeframes,
                    lifecycle: WidgetLifecycle::Open,
                    host_mode: WidgetHostMode::DetachedNativeViewport,
                    rect: WorkspaceLayoutRect::new(80.0, 120.0, 900.0, 480.0),
                    z_order: 12,
                },
            ],
        });

        assert_eq!(shell.active_widget_id(), Some(WidgetId::new(5)));
        assert_eq!(
            shell.focus_owner(),
            ShellFocusOwner::Widget(WidgetId::new(5))
        );
        let layout = shell.workspace_layout_snapshot();
        assert_eq!(layout.next_widget_id, 6);
        assert_eq!(layout.next_z_order, 13);
        assert_eq!(layout.widgets.len(), 2);
        assert_eq!(layout.widgets[0].id, WidgetId::new(4));
        assert_eq!(layout.widgets[0].lifecycle, WidgetLifecycle::Minimized);
        assert_eq!(layout.widgets[1].id, WidgetId::new(5));
        assert_eq!(
            layout.widgets[1].host_mode,
            WidgetHostMode::DetachedNativeViewport
        );
        assert_eq!(layout.widgets[1].rect.width, 900.0);
    }

    #[test]
    fn workspace_layout_reset_returns_shell_to_empty_defaults() {
        let mut shell = WorkspaceShell::default();
        shell.spawn_widget(WidgetKind::LiveChart);
        shell.reset_workspace_layout();

        assert_eq!(shell.focus_owner(), ShellFocusOwner::Shell);
        assert!(!shell.has_workspace_widgets());
        assert_eq!(shell.workspace_layout_snapshot().next_widget_id, 1);
        assert_eq!(shell.workspace_layout_snapshot().next_z_order, 1);
    }

    #[test]
    fn widget_layout_rect_updates_durable_geometry() {
        let mut shell = WorkspaceShell::default();
        shell.spawn_widget(WidgetKind::LiveChart);

        assert!(shell.set_widget_layout_rect(
            WidgetId::new(1),
            WorkspaceLayoutRect::new(120.0, 140.0, 800.0, 460.0)
        ));
        let layout = shell.workspace_layout_snapshot();
        assert_eq!(layout.widgets[0].rect.x, 120.0);
        assert_eq!(layout.widgets[0].rect.y, 140.0);
        assert_eq!(layout.widgets[0].rect.width, 800.0);
        assert_eq!(layout.widgets[0].rect.height, 460.0);
        assert!(!shell.set_widget_layout_rect(
            WidgetId::new(1),
            WorkspaceLayoutRect::new(120.1, 140.1, 800.1, 460.1)
        ));
    }

    #[test]
    fn workspace_preset_applies_deterministic_widget_layouts() {
        let mut shell = WorkspaceShell::default();
        shell.spawn_widget(WidgetKind::News);

        let descriptors = shell.apply_workspace_preset(WorkspacePresetKind::TwoLiveCharts);

        assert_eq!(
            descriptors,
            vec![
                WidgetDescriptor::new(WidgetId::new(1), WidgetKind::LiveChart),
                WidgetDescriptor::new(WidgetId::new(2), WidgetKind::LiveChart),
            ]
        );
        let layout = shell.workspace_layout_snapshot();
        assert_eq!(layout.widgets.len(), 2);
        assert_eq!(layout.next_widget_id, 3);
        assert_eq!(layout.next_z_order, 3);
        assert_eq!(layout.active_widget_id, Some(WidgetId::new(2)));
        assert_eq!(layout.widgets[0].rect.width, 680.0);
        assert_eq!(layout.widgets[1].rect.x, 772.0);
    }
}
