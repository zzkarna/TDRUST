use super::{pane::PaneId, theme::ChartColor};

pub const DRAWING_SCHEMA_VERSION: u32 = 2;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct DrawingId(pub u64);

impl DrawingId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct DrawingGroupId(pub u64);

impl DrawingGroupId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum DrawingOwner {
    MainSeries,
    Study(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum OhlcField {
    Open,
    High,
    Low,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum DrawingKind {
    HorizontalLine,
    HorizontalRay,
    VerticalLine,
    TrendLine,
    Ray,
    Brush,
    FibRetracement,
    LongPosition,
    ShortPosition,
    Rectangle,
    Text,
    AnchoredText,
    PriceNote,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum DrawingAnchor {
    TimePrice {
        time: i64,
        price: f64,
    },
    TimeOnly {
        time: i64,
    },
    PriceOnly {
        price: f64,
    },
    LogicalPrice {
        logical: f64,
        price: f64,
    },
    Percent {
        x: f32,
        y: f32,
    },
    StickedOhlc {
        time: i64,
        field: OhlcField,
        price: f64,
        source_interval_seconds: Option<i64>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum DrawingLineStyle {
    Solid,
    Dashed,
    Dotted,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DrawingStyle {
    pub stroke_color: ChartColor,
    pub stroke_width: f32,
    pub line_style: DrawingLineStyle,
    pub fill_color: Option<ChartColor>,
    pub text_color: ChartColor,
    pub text_size: f32,
}

impl Default for DrawingStyle {
    fn default() -> Self {
        Self {
            stroke_color: ChartColor::rgb(242, 201, 76),
            stroke_width: 1.5,
            line_style: DrawingLineStyle::Solid,
            fill_color: None,
            text_color: ChartColor::rgb(235, 240, 245),
            text_size: 13.0,
        }
    }
}

impl DrawingStyle {
    pub fn is_valid(&self) -> bool {
        self.stroke_width.is_finite()
            && self.stroke_width > 0.0
            && self.text_size.is_finite()
            && self.text_size > 0.0
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum DrawingPayload {
    FibRetracement(FibRetracementSettings),
    Position(PositionDrawingSettings),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PositionDrawingSettings {
    #[serde(default = "default_true")]
    pub labels_visible: bool,
    #[serde(default = "default_true")]
    pub prices_visible: bool,
    #[serde(default = "default_position_profit_color")]
    pub profit_fill_color: ChartColor,
    #[serde(default = "default_position_risk_color")]
    pub risk_fill_color: ChartColor,
}

impl Default for PositionDrawingSettings {
    fn default() -> Self {
        Self {
            labels_visible: true,
            prices_visible: true,
            profit_fill_color: default_position_profit_color(),
            risk_fill_color: default_position_risk_color(),
        }
    }
}

impl PositionDrawingSettings {
    pub const fn is_valid(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FibRetracementSettings {
    #[serde(default = "default_true")]
    pub trend_line_visible: bool,
    #[serde(default = "default_true")]
    pub labels_visible: bool,
    #[serde(default = "default_true")]
    pub prices_visible: bool,
    #[serde(default)]
    pub label_position: FibRetracementLabelPosition,
    #[serde(default)]
    pub label_side: FibRetracementLabelSide,
    #[serde(default)]
    pub reverse: bool,
    #[serde(default)]
    pub background_visible: bool,
    #[serde(default = "default_fib_retracement_background_color")]
    pub background_color: ChartColor,
    #[serde(default)]
    pub extend_left: bool,
    #[serde(default)]
    pub extend_right: bool,
    #[serde(default = "default_fib_retracement_levels")]
    pub levels: Vec<FibRetracementLevel>,
}

impl Default for FibRetracementSettings {
    fn default() -> Self {
        Self {
            trend_line_visible: true,
            labels_visible: true,
            prices_visible: true,
            label_position: FibRetracementLabelPosition::default(),
            label_side: FibRetracementLabelSide::default(),
            reverse: false,
            background_visible: false,
            background_color: default_fib_retracement_background_color(),
            extend_left: false,
            extend_right: false,
            levels: default_fib_retracement_levels(),
        }
    }
}

impl FibRetracementSettings {
    pub fn is_valid(&self) -> bool {
        self.levels.iter().all(FibRetracementLevel::is_valid)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum FibRetracementLabelPosition {
    Above,
    #[default]
    OnLine,
    Below,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum FibRetracementLabelSide {
    #[default]
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FibRetracementLevel {
    pub value: f64,
    #[serde(default = "default_true")]
    pub visible: bool,
    pub color: ChartColor,
}

impl FibRetracementLevel {
    pub const fn new(value: f64, color: ChartColor) -> Self {
        Self {
            value,
            visible: true,
            color,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.value.is_finite()
    }
}

pub const DEFAULT_FIB_RETRACEMENT_LEVELS: [FibRetracementLevel; 7] = [
    FibRetracementLevel::new(0.0, ChartColor::rgb(142, 148, 156)),
    FibRetracementLevel::new(0.236, ChartColor::rgb(242, 80, 80)),
    FibRetracementLevel::new(0.382, ChartColor::rgb(255, 152, 0)),
    FibRetracementLevel::new(0.5, ChartColor::rgb(76, 175, 80)),
    FibRetracementLevel::new(0.618, ChartColor::rgb(0, 150, 136)),
    FibRetracementLevel::new(0.786, ChartColor::rgb(86, 166, 222)),
    FibRetracementLevel::new(1.0, ChartColor::rgb(142, 148, 156)),
];

fn default_fib_retracement_levels() -> Vec<FibRetracementLevel> {
    DEFAULT_FIB_RETRACEMENT_LEVELS.to_vec()
}

fn default_fib_retracement_background_color() -> ChartColor {
    ChartColor::rgba(41, 98, 255, 34)
}

fn default_position_profit_color() -> ChartColor {
    ChartColor::rgba(46, 204, 113, 58)
}

fn default_position_risk_color() -> ChartColor {
    ChartColor::rgba(235, 87, 87, 58)
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DrawingSpec {
    pub schema_version: u32,
    pub id: DrawingId,
    pub kind: DrawingKind,
    pub pane: PaneId,
    pub symbol: Option<String>,
    pub owner: Option<DrawingOwner>,
    pub group_id: Option<DrawingGroupId>,
    pub anchors: Vec<DrawingAnchor>,
    pub style: DrawingStyle,
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<DrawingPayload>,
    pub visible: bool,
    pub locked: bool,
    pub selectable: bool,
    pub z_order: i32,
}

impl DrawingSpec {
    pub fn new(
        id: DrawingId,
        kind: DrawingKind,
        pane: PaneId,
        anchors: Vec<DrawingAnchor>,
    ) -> Self {
        Self {
            schema_version: DRAWING_SCHEMA_VERSION,
            id,
            kind,
            pane,
            symbol: None,
            owner: None,
            group_id: None,
            anchors,
            style: DrawingStyle::default(),
            text: None,
            payload: default_payload_for_kind(kind),
            visible: true,
            locked: false,
            selectable: true,
            z_order: 0,
        }
    }

    pub fn with_symbol(mut self, symbol: impl Into<String>) -> Self {
        self.symbol = Some(symbol.into());
        self
    }

    pub fn with_owner(mut self, owner: DrawingOwner) -> Self {
        self.owner = Some(owner);
        self
    }

    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    pub fn with_payload(mut self, payload: DrawingPayload) -> Self {
        self.payload = Some(payload);
        self
    }

    pub fn fib_retracement_settings(&self) -> FibRetracementSettings {
        match &self.payload {
            Some(DrawingPayload::FibRetracement(settings)) => settings.clone(),
            None => FibRetracementSettings::default(),
            Some(DrawingPayload::Position(_)) => FibRetracementSettings::default(),
        }
    }

    pub fn position_settings(&self) -> PositionDrawingSettings {
        match &self.payload {
            Some(DrawingPayload::Position(settings)) => settings.clone(),
            None => PositionDrawingSettings::default(),
            Some(DrawingPayload::FibRetracement(_)) => PositionDrawingSettings::default(),
        }
    }

    pub fn with_visibility(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }

    pub fn with_locked(mut self, locked: bool) -> Self {
        self.locked = locked;
        self
    }

    pub fn with_selectable(mut self, selectable: bool) -> Self {
        self.selectable = selectable;
        self
    }

    pub fn with_z_order(mut self, z_order: i32) -> Self {
        self.z_order = z_order;
        self
    }

    pub fn validate(&self) -> Result<(), DrawingValidationError> {
        validate_drawing(self)
    }
}

fn default_payload_for_kind(kind: DrawingKind) -> Option<DrawingPayload> {
    match kind {
        DrawingKind::FibRetracement => Some(DrawingPayload::FibRetracement(
            FibRetracementSettings::default(),
        )),
        DrawingKind::LongPosition | DrawingKind::ShortPosition => {
            Some(DrawingPayload::Position(PositionDrawingSettings::default()))
        }
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DrawingGroupSpec {
    pub id: DrawingGroupId,
    pub pane: PaneId,
    pub symbol: Option<String>,
    pub drawing_ids: Vec<DrawingId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DrawingStore {
    drawings: Vec<DrawingSpec>,
    groups: Vec<DrawingGroupSpec>,
    next_drawing_id: u64,
    next_group_id: u64,
    generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawingBulkAction {
    SetVisible(bool),
    SetLocked(bool),
    SetSelectable(bool),
    Delete,
    RemoveFromGroup,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrawingBulkActionOutcome {
    pub id: DrawingId,
    pub status: DrawingBulkActionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrawingBulkActionStatus {
    Changed,
    Unchanged,
    Deleted,
    Error(DrawingEditError),
}

impl Default for DrawingStore {
    fn default() -> Self {
        Self {
            drawings: Vec::new(),
            groups: Vec::new(),
            next_drawing_id: 1,
            next_group_id: 1,
            generation: 0,
        }
    }
}

impl DrawingStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn drawings(&self) -> &[DrawingSpec] {
        &self.drawings
    }

    pub fn groups(&self) -> &[DrawingGroupSpec] {
        &self.groups
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn restore_from(&mut self, mut snapshot: Self) {
        snapshot.generation = self.generation.wrapping_add(1);
        *self = snapshot;
    }

    pub fn next_drawing_id(&self) -> u64 {
        self.next_drawing_id
    }

    pub fn next_group_id(&self) -> u64 {
        self.next_group_id
    }

    pub fn from_persisted_parts(
        drawings: Vec<DrawingSpec>,
        groups: Vec<DrawingGroupSpec>,
        next_drawing_id: u64,
        next_group_id: u64,
    ) -> Result<Self, DrawingValidationError> {
        let mut store = Self::new();
        for drawing in drawings {
            store.insert(drawing)?;
        }
        for group in groups {
            store.insert_persisted_group(group)?;
        }
        for drawing in &store.drawings {
            if let Some(group_id) = drawing.group_id
                && store.group_index(group_id).is_none()
            {
                return Err(DrawingValidationError::MissingGroup(group_id));
            }
        }
        store.next_drawing_id = store.next_drawing_id.max(next_drawing_id.max(1));
        store.next_group_id = store.next_group_id.max(next_group_id.max(1));
        store.generation = 0;
        Ok(store)
    }

    pub fn get(&self, id: DrawingId) -> Option<&DrawingSpec> {
        self.drawings.iter().find(|drawing| drawing.id == id)
    }

    pub fn visible(&self) -> impl Iterator<Item = &DrawingSpec> {
        self.drawings.iter().filter(|drawing| drawing.visible)
    }

    pub fn create(
        &mut self,
        kind: DrawingKind,
        pane: PaneId,
        anchors: Vec<DrawingAnchor>,
    ) -> Result<DrawingId, DrawingValidationError> {
        self.create_with_symbol(kind, pane, anchors, None)
    }

    pub fn create_with_symbol(
        &mut self,
        kind: DrawingKind,
        pane: PaneId,
        anchors: Vec<DrawingAnchor>,
        symbol: Option<String>,
    ) -> Result<DrawingId, DrawingValidationError> {
        let id = DrawingId::new(self.next_drawing_id.max(1));
        let mut spec = DrawingSpec::new(id, kind, pane, anchors);
        spec.symbol = symbol;
        self.insert(spec)
    }

    pub fn insert(&mut self, spec: DrawingSpec) -> Result<DrawingId, DrawingValidationError> {
        if spec.id.0 == 0 {
            return Err(DrawingValidationError::InvalidDrawingId(spec.id));
        }
        if self.get(spec.id).is_some() {
            return Err(DrawingValidationError::DuplicateDrawingId(spec.id));
        }
        spec.validate()?;
        let id = spec.id;
        self.next_drawing_id = self.next_drawing_id.max(id.0.saturating_add(1));
        self.drawings.push(spec);
        self.sort_drawings();
        self.bump_generation();
        Ok(id)
    }

    pub fn insert_copy(
        &mut self,
        mut spec: DrawingSpec,
    ) -> Result<DrawingId, DrawingValidationError> {
        let id = DrawingId::new(self.next_drawing_id.max(1));
        spec.id = id;
        spec.group_id = None;
        self.insert(spec)
    }

    pub fn set_visible(&mut self, id: DrawingId, visible: bool) -> bool {
        let Some(drawing) = self.drawings.iter_mut().find(|drawing| drawing.id == id) else {
            return false;
        };
        if drawing.visible == visible {
            return false;
        }
        drawing.visible = visible;
        self.bump_generation();
        true
    }

    pub fn set_locked(&mut self, id: DrawingId, locked: bool) -> bool {
        let Some(drawing) = self.drawings.iter_mut().find(|drawing| drawing.id == id) else {
            return false;
        };
        if drawing.locked == locked {
            return false;
        }
        drawing.locked = locked;
        self.bump_generation();
        true
    }

    pub fn set_selectable(&mut self, id: DrawingId, selectable: bool) -> bool {
        let Some(drawing) = self.drawings.iter_mut().find(|drawing| drawing.id == id) else {
            return false;
        };
        if drawing.selectable == selectable {
            return false;
        }
        drawing.selectable = selectable;
        self.bump_generation();
        true
    }

    pub fn replace_anchors(
        &mut self,
        id: DrawingId,
        anchors: Vec<DrawingAnchor>,
    ) -> Result<(), DrawingEditError> {
        let Some(index) = self.drawing_index(id) else {
            return Err(DrawingEditError::MissingDrawing(id));
        };
        if self.drawings[index].locked {
            return Err(DrawingEditError::LockedDrawing(id));
        }
        let mut candidate = self.drawings[index].clone();
        candidate.anchors = anchors;
        candidate.validate().map_err(DrawingEditError::Invalid)?;
        if self.drawings[index].anchors == candidate.anchors {
            return Ok(());
        }
        self.drawings[index].anchors = candidate.anchors;
        self.bump_generation();
        Ok(())
    }

    pub fn replace_style(
        &mut self,
        id: DrawingId,
        style: DrawingStyle,
    ) -> Result<(), DrawingEditError> {
        let Some(index) = self.drawing_index(id) else {
            return Err(DrawingEditError::MissingDrawing(id));
        };
        if self.drawings[index].locked {
            return Err(DrawingEditError::LockedDrawing(id));
        }
        let mut candidate = self.drawings[index].clone();
        candidate.style = style;
        candidate.validate().map_err(DrawingEditError::Invalid)?;
        if self.drawings[index].style == candidate.style {
            return Ok(());
        }
        self.drawings[index].style = candidate.style;
        self.bump_generation();
        Ok(())
    }

    pub fn set_text(
        &mut self,
        id: DrawingId,
        text: Option<String>,
    ) -> Result<(), DrawingEditError> {
        let Some(index) = self.drawing_index(id) else {
            return Err(DrawingEditError::MissingDrawing(id));
        };
        if self.drawings[index].locked {
            return Err(DrawingEditError::LockedDrawing(id));
        }
        if self.drawings[index].text == text {
            return Ok(());
        }
        self.drawings[index].text = text;
        self.bump_generation();
        Ok(())
    }

    pub fn replace_payload(
        &mut self,
        id: DrawingId,
        payload: Option<DrawingPayload>,
    ) -> Result<(), DrawingEditError> {
        let Some(index) = self.drawing_index(id) else {
            return Err(DrawingEditError::MissingDrawing(id));
        };
        if self.drawings[index].locked {
            return Err(DrawingEditError::LockedDrawing(id));
        }
        let mut candidate = self.drawings[index].clone();
        candidate.payload = payload;
        candidate.validate().map_err(DrawingEditError::Invalid)?;
        if self.drawings[index].payload == candidate.payload {
            return Ok(());
        }
        self.drawings[index].payload = candidate.payload;
        self.bump_generation();
        Ok(())
    }

    pub fn remove(&mut self, id: DrawingId) -> Result<(), DrawingEditError> {
        let Some(index) = self.drawing_index(id) else {
            return Err(DrawingEditError::MissingDrawing(id));
        };
        if self.drawings[index].locked {
            return Err(DrawingEditError::LockedDrawing(id));
        }
        self.drawings.remove(index);
        self.remove_deleted_drawing_from_groups(id);
        self.bump_generation();
        Ok(())
    }

    pub fn normalize_z_order(&mut self) -> bool {
        let order = self.normalized_drawing_order();
        let changed = self.assign_z_order_for_order(&order);
        if changed {
            self.sort_drawings();
            self.bump_generation();
        }
        changed
    }

    pub fn bring_forward(&mut self, id: DrawingId) -> Result<bool, DrawingEditError> {
        let Some(index) = self.z_order_index(id)? else {
            return Ok(false);
        };
        let target = index.saturating_add(1).min(self.drawings.len() - 1);
        self.reorder_z_order(id, target)
    }

    pub fn send_backward(&mut self, id: DrawingId) -> Result<bool, DrawingEditError> {
        let Some(index) = self.z_order_index(id)? else {
            return Ok(false);
        };
        self.reorder_z_order(id, index.saturating_sub(1))
    }

    pub fn bring_to_front(&mut self, id: DrawingId) -> Result<bool, DrawingEditError> {
        if self.drawings.is_empty() {
            return Err(DrawingEditError::MissingDrawing(id));
        }
        self.reorder_z_order(id, self.drawings.len() - 1)
    }

    pub fn send_to_back(&mut self, id: DrawingId) -> Result<bool, DrawingEditError> {
        self.reorder_z_order(id, 0)
    }

    pub fn create_group(
        &mut self,
        drawing_ids: Vec<DrawingId>,
    ) -> Result<DrawingGroupId, DrawingValidationError> {
        self.validate_group_members(&drawing_ids)?;
        let id = self.allocate_group_id();
        let first = self
            .get(drawing_ids[0])
            .expect("group validation requires existing drawings");
        let group = DrawingGroupSpec {
            id,
            pane: first.pane,
            symbol: first.symbol.clone(),
            drawing_ids,
        };
        for drawing_id in &group.drawing_ids {
            if let Some(drawing) = self
                .drawings
                .iter_mut()
                .find(|drawing| drawing.id == *drawing_id)
            {
                drawing.group_id = Some(id);
            }
        }
        self.groups.push(group);
        self.bump_generation();
        Ok(id)
    }

    pub fn ungroup(
        &mut self,
        group_id: DrawingGroupId,
    ) -> Result<Vec<DrawingId>, DrawingEditError> {
        let Some(index) = self.group_index(group_id) else {
            return Err(DrawingEditError::MissingGroup(group_id));
        };
        let group = self.groups.remove(index);
        for drawing_id in &group.drawing_ids {
            if let Some(drawing) = self
                .drawings
                .iter_mut()
                .find(|drawing| drawing.id == *drawing_id)
                && drawing.group_id == Some(group_id)
            {
                drawing.group_id = None;
            }
        }
        self.bump_generation();
        Ok(group.drawing_ids)
    }

    pub fn remove_from_group(
        &mut self,
        drawing_id: DrawingId,
    ) -> Result<Option<DrawingGroupId>, DrawingEditError> {
        let Some(drawing_index) = self.drawing_index(drawing_id) else {
            return Err(DrawingEditError::MissingDrawing(drawing_id));
        };
        let Some(group_id) = self.drawings[drawing_index].group_id else {
            return Ok(None);
        };
        let Some(group_index) = self.group_index(group_id) else {
            return Err(DrawingEditError::MissingGroup(group_id));
        };

        self.groups[group_index]
            .drawing_ids
            .retain(|member_id| *member_id != drawing_id);
        self.drawings[drawing_index].group_id = None;

        if self.groups[group_index].drawing_ids.len() < 2 {
            let group = self.groups.remove(group_index);
            for member_id in group.drawing_ids {
                if let Some(member) = self
                    .drawings
                    .iter_mut()
                    .find(|drawing| drawing.id == member_id)
                {
                    member.group_id = None;
                }
            }
        }

        self.bump_generation();
        Ok(Some(group_id))
    }

    pub fn apply_bulk_action(
        &mut self,
        ids: &[DrawingId],
        action: DrawingBulkAction,
    ) -> Vec<DrawingBulkActionOutcome> {
        ids.iter()
            .copied()
            .map(|id| DrawingBulkActionOutcome {
                id,
                status: self.apply_bulk_action_one(id, action),
            })
            .collect()
    }

    fn allocate_group_id(&mut self) -> DrawingGroupId {
        let id = DrawingGroupId::new(self.next_group_id.max(1));
        self.next_group_id = id.0.saturating_add(1);
        id
    }

    fn insert_persisted_group(
        &mut self,
        group: DrawingGroupSpec,
    ) -> Result<(), DrawingValidationError> {
        if group.id.0 == 0 {
            return Err(DrawingValidationError::InvalidGroupId(group.id));
        }
        if self.group_index(group.id).is_some() {
            return Err(DrawingValidationError::DuplicateGroupId(group.id));
        }
        self.validate_group_members_for_group(&group)?;
        for drawing_id in &group.drawing_ids {
            if let Some(drawing) = self
                .drawings
                .iter_mut()
                .find(|drawing| drawing.id == *drawing_id)
            {
                drawing.group_id = Some(group.id);
            }
        }
        self.next_group_id = self.next_group_id.max(group.id.0.saturating_add(1));
        self.groups.push(group);
        self.bump_generation();
        Ok(())
    }

    fn drawing_index(&self, id: DrawingId) -> Option<usize> {
        self.drawings.iter().position(|drawing| drawing.id == id)
    }

    fn group_index(&self, id: DrawingGroupId) -> Option<usize> {
        self.groups.iter().position(|group| group.id == id)
    }

    fn validate_group_members(
        &self,
        drawing_ids: &[DrawingId],
    ) -> Result<(), DrawingValidationError> {
        if drawing_ids.is_empty() {
            return Err(DrawingValidationError::EmptyGroup);
        }

        for (index, drawing_id) in drawing_ids.iter().enumerate() {
            if drawing_ids[..index].contains(drawing_id) {
                return Err(DrawingValidationError::DuplicateGroupMember(*drawing_id));
            }
        }

        let first = self
            .get(drawing_ids[0])
            .ok_or(DrawingValidationError::MissingDrawing(drawing_ids[0]))?;
        if first.group_id.is_some() {
            return Err(DrawingValidationError::DrawingAlreadyGrouped(first.id));
        }
        let pane = first.pane;
        let symbol = &first.symbol;

        for drawing_id in &drawing_ids[1..] {
            let drawing = self
                .get(*drawing_id)
                .ok_or(DrawingValidationError::MissingDrawing(*drawing_id))?;
            if drawing.group_id.is_some() {
                return Err(DrawingValidationError::DrawingAlreadyGrouped(*drawing_id));
            }
            if drawing.pane != pane {
                return Err(DrawingValidationError::CrossPaneGroup);
            }
            if &drawing.symbol != symbol {
                return Err(DrawingValidationError::CrossSymbolGroup);
            }
        }

        if drawing_ids.len() < 2 {
            return Err(DrawingValidationError::TooFewGroupMembers);
        }

        Ok(())
    }

    fn validate_group_members_for_group(
        &self,
        group: &DrawingGroupSpec,
    ) -> Result<(), DrawingValidationError> {
        if group.drawing_ids.is_empty() {
            return Err(DrawingValidationError::EmptyGroup);
        }
        if group.drawing_ids.len() < 2 {
            return Err(DrawingValidationError::TooFewGroupMembers);
        }

        for (index, drawing_id) in group.drawing_ids.iter().enumerate() {
            if group.drawing_ids[..index].contains(drawing_id) {
                return Err(DrawingValidationError::DuplicateGroupMember(*drawing_id));
            }
        }

        for drawing_id in &group.drawing_ids {
            let drawing = self
                .get(*drawing_id)
                .ok_or(DrawingValidationError::MissingDrawing(*drawing_id))?;
            if drawing.pane != group.pane {
                return Err(DrawingValidationError::CrossPaneGroup);
            }
            if drawing.symbol != group.symbol {
                return Err(DrawingValidationError::CrossSymbolGroup);
            }
            if drawing
                .group_id
                .is_some_and(|drawing_group| drawing_group != group.id)
            {
                return Err(DrawingValidationError::DrawingAlreadyGrouped(*drawing_id));
            }
        }

        Ok(())
    }

    fn z_order_index(&self, id: DrawingId) -> Result<Option<usize>, DrawingEditError> {
        if self.get(id).is_none() {
            return Err(DrawingEditError::MissingDrawing(id));
        }
        Ok(self
            .normalized_drawing_order()
            .iter()
            .position(|drawing_id| *drawing_id == id))
    }

    fn reorder_z_order(
        &mut self,
        id: DrawingId,
        target_index: usize,
    ) -> Result<bool, DrawingEditError> {
        if self.get(id).is_none() {
            return Err(DrawingEditError::MissingDrawing(id));
        }
        let mut order = self.normalized_drawing_order();
        let Some(current_index) = order.iter().position(|drawing_id| *drawing_id == id) else {
            return Err(DrawingEditError::MissingDrawing(id));
        };
        let target_index = target_index.min(order.len().saturating_sub(1));
        if current_index != target_index {
            let drawing_id = order.remove(current_index);
            order.insert(target_index, drawing_id);
        }
        let changed = self.assign_z_order_for_order(&order);
        if changed {
            self.sort_drawings();
            self.bump_generation();
        }
        Ok(changed)
    }

    fn normalized_drawing_order(&self) -> Vec<DrawingId> {
        let mut drawings = self.drawings.iter().collect::<Vec<_>>();
        drawings.sort_by_key(|drawing| (drawing.z_order, drawing.id));
        drawings.into_iter().map(|drawing| drawing.id).collect()
    }

    fn assign_z_order_for_order(&mut self, order: &[DrawingId]) -> bool {
        let mut changed = false;
        for (index, drawing_id) in order.iter().copied().enumerate() {
            let next = (index as i64).min(i64::from(i32::MAX)) as i32;
            if let Some(drawing) = self
                .drawings
                .iter_mut()
                .find(|drawing| drawing.id == drawing_id)
                && drawing.z_order != next
            {
                drawing.z_order = next;
                changed = true;
            }
        }
        changed
    }

    fn remove_deleted_drawing_from_groups(&mut self, id: DrawingId) {
        for group in &mut self.groups {
            group.drawing_ids.retain(|drawing_id| *drawing_id != id);
        }

        let groups_to_remove = self
            .groups
            .iter()
            .filter(|group| group.drawing_ids.len() < 2)
            .map(|group| group.id)
            .collect::<Vec<_>>();
        if groups_to_remove.is_empty() {
            return;
        }

        self.groups
            .retain(|group| !groups_to_remove.contains(&group.id));
        for drawing in &mut self.drawings {
            if drawing
                .group_id
                .is_some_and(|group_id| groups_to_remove.contains(&group_id))
            {
                drawing.group_id = None;
            }
        }
    }

    fn apply_bulk_action_one(
        &mut self,
        id: DrawingId,
        action: DrawingBulkAction,
    ) -> DrawingBulkActionStatus {
        if self.get(id).is_none() {
            return DrawingBulkActionStatus::Error(DrawingEditError::MissingDrawing(id));
        }
        match action {
            DrawingBulkAction::SetVisible(visible) => changed_status(self.set_visible(id, visible)),
            DrawingBulkAction::SetLocked(locked) => changed_status(self.set_locked(id, locked)),
            DrawingBulkAction::SetSelectable(selectable) => {
                changed_status(self.set_selectable(id, selectable))
            }
            DrawingBulkAction::Delete => match self.remove(id) {
                Ok(()) => DrawingBulkActionStatus::Deleted,
                Err(error) => DrawingBulkActionStatus::Error(error),
            },
            DrawingBulkAction::RemoveFromGroup => match self.remove_from_group(id) {
                Ok(Some(_)) => DrawingBulkActionStatus::Changed,
                Ok(None) => DrawingBulkActionStatus::Unchanged,
                Err(error) => DrawingBulkActionStatus::Error(error),
            },
        }
    }

    fn sort_drawings(&mut self) {
        self.drawings
            .sort_by_key(|drawing| (drawing.z_order, drawing.id));
    }

    fn bump_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrawingValidationError {
    UnsupportedSchemaVersion(u32),
    InvalidDrawingId(DrawingId),
    InvalidGroupId(DrawingGroupId),
    DuplicateDrawingId(DrawingId),
    DuplicateGroupId(DrawingGroupId),
    MissingDrawing(DrawingId),
    MissingGroup(DrawingGroupId),
    WrongAnchorCount {
        kind: DrawingKind,
        expected: &'static str,
        actual: usize,
    },
    UnsupportedAnchor {
        kind: DrawingKind,
        anchor_index: usize,
    },
    NonFiniteAnchor {
        anchor_index: usize,
    },
    PercentAnchorOutOfRange {
        anchor_index: usize,
    },
    InvalidStyle,
    InvalidPayload {
        kind: DrawingKind,
    },
    InvalidFibLevel {
        level_index: usize,
    },
    InvalidPositionAnchors {
        kind: DrawingKind,
    },
    EmptyGroup,
    TooFewGroupMembers,
    DuplicateGroupMember(DrawingId),
    DrawingAlreadyGrouped(DrawingId),
    CrossPaneGroup,
    CrossSymbolGroup,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrawingEditError {
    MissingDrawing(DrawingId),
    MissingGroup(DrawingGroupId),
    LockedDrawing(DrawingId),
    Invalid(DrawingValidationError),
}

fn changed_status(changed: bool) -> DrawingBulkActionStatus {
    if changed {
        DrawingBulkActionStatus::Changed
    } else {
        DrawingBulkActionStatus::Unchanged
    }
}

fn validate_drawing(spec: &DrawingSpec) -> Result<(), DrawingValidationError> {
    if spec.schema_version != DRAWING_SCHEMA_VERSION {
        return Err(DrawingValidationError::UnsupportedSchemaVersion(
            spec.schema_version,
        ));
    }
    if !spec.style.is_valid() {
        return Err(DrawingValidationError::InvalidStyle);
    }
    for (index, anchor) in spec.anchors.iter().enumerate() {
        validate_anchor(*anchor, index)?;
    }

    match spec.kind {
        DrawingKind::HorizontalLine => {
            validate_anchor_count(spec, "1", 1)?;
            validate_anchor_predicate(spec, has_price_coordinate)?;
        }
        DrawingKind::HorizontalRay => {
            validate_anchor_count(spec, "1", 1)?;
            validate_anchor_predicate(spec, is_model_point)?;
        }
        DrawingKind::VerticalLine => {
            validate_anchor_count(spec, "1", 1)?;
            validate_anchor_predicate(spec, has_time_coordinate)?;
        }
        DrawingKind::TrendLine
        | DrawingKind::Ray
        | DrawingKind::FibRetracement
        | DrawingKind::Rectangle => {
            validate_anchor_count(spec, "2", 2)?;
            validate_anchor_predicate(spec, is_model_point)?;
        }
        DrawingKind::LongPosition | DrawingKind::ShortPosition => {
            validate_anchor_count(spec, "3", 3)?;
            validate_anchor_predicate(spec, is_model_point)?;
            validate_position_anchor_direction(spec)?;
        }
        DrawingKind::Brush => {
            validate_min_anchor_count(spec, "2+", 2)?;
            validate_anchor_predicate(spec, is_model_point)?;
        }
        DrawingKind::Text | DrawingKind::PriceNote => {
            validate_anchor_count(spec, "1", 1)?;
            validate_anchor_predicate(spec, is_model_or_price_point)?;
        }
        DrawingKind::AnchoredText => {
            validate_anchor_count(spec, "1", 1)?;
            validate_anchor_predicate(spec, is_percent_anchor)?;
        }
    }

    validate_payload(spec)?;
    Ok(())
}

fn validate_payload(spec: &DrawingSpec) -> Result<(), DrawingValidationError> {
    match (spec.kind, &spec.payload) {
        (DrawingKind::FibRetracement, Some(DrawingPayload::FibRetracement(settings))) => {
            validate_fib_retracement_settings(settings)
        }
        (DrawingKind::FibRetracement, None) => Ok(()),
        (
            DrawingKind::LongPosition | DrawingKind::ShortPosition,
            Some(DrawingPayload::Position(settings)),
        ) => validate_position_settings(settings),
        (DrawingKind::LongPosition | DrawingKind::ShortPosition, None) => Ok(()),
        (_, None) => Ok(()),
        (kind, Some(_)) => Err(DrawingValidationError::InvalidPayload { kind }),
    }
}

fn validate_fib_retracement_settings(
    settings: &FibRetracementSettings,
) -> Result<(), DrawingValidationError> {
    for (index, level) in settings.levels.iter().enumerate() {
        if !level.is_valid() {
            return Err(DrawingValidationError::InvalidFibLevel { level_index: index });
        }
    }
    Ok(())
}

fn validate_position_settings(
    settings: &PositionDrawingSettings,
) -> Result<(), DrawingValidationError> {
    if settings.is_valid() {
        Ok(())
    } else {
        Err(DrawingValidationError::InvalidPayload {
            kind: DrawingKind::LongPosition,
        })
    }
}

fn validate_position_anchor_direction(spec: &DrawingSpec) -> Result<(), DrawingValidationError> {
    let Some(entry) = spec
        .anchors
        .first()
        .and_then(|anchor| drawing_anchor_price(*anchor))
    else {
        return Err(DrawingValidationError::UnsupportedAnchor {
            kind: spec.kind,
            anchor_index: 0,
        });
    };
    let Some(target) = spec
        .anchors
        .get(1)
        .and_then(|anchor| drawing_anchor_price(*anchor))
    else {
        return Err(DrawingValidationError::UnsupportedAnchor {
            kind: spec.kind,
            anchor_index: 1,
        });
    };
    let Some(stop) = spec
        .anchors
        .get(2)
        .and_then(|anchor| drawing_anchor_price(*anchor))
    else {
        return Err(DrawingValidationError::UnsupportedAnchor {
            kind: spec.kind,
            anchor_index: 2,
        });
    };

    let valid = match spec.kind {
        DrawingKind::LongPosition => target > entry && stop < entry,
        DrawingKind::ShortPosition => target < entry && stop > entry,
        _ => true,
    };
    if valid {
        Ok(())
    } else {
        Err(DrawingValidationError::InvalidPositionAnchors { kind: spec.kind })
    }
}

fn drawing_anchor_price(anchor: DrawingAnchor) -> Option<f64> {
    match anchor {
        DrawingAnchor::TimePrice { price, .. }
        | DrawingAnchor::LogicalPrice { price, .. }
        | DrawingAnchor::StickedOhlc { price, .. } => Some(price),
        DrawingAnchor::TimeOnly { .. }
        | DrawingAnchor::PriceOnly { .. }
        | DrawingAnchor::Percent { .. } => None,
    }
}

fn validate_anchor_count(
    spec: &DrawingSpec,
    expected: &'static str,
    count: usize,
) -> Result<(), DrawingValidationError> {
    if spec.anchors.len() == count {
        Ok(())
    } else {
        Err(DrawingValidationError::WrongAnchorCount {
            kind: spec.kind,
            expected,
            actual: spec.anchors.len(),
        })
    }
}

fn validate_min_anchor_count(
    spec: &DrawingSpec,
    expected: &'static str,
    count: usize,
) -> Result<(), DrawingValidationError> {
    if spec.anchors.len() >= count {
        Ok(())
    } else {
        Err(DrawingValidationError::WrongAnchorCount {
            kind: spec.kind,
            expected,
            actual: spec.anchors.len(),
        })
    }
}

fn validate_anchor_predicate(
    spec: &DrawingSpec,
    predicate: fn(DrawingAnchor) -> bool,
) -> Result<(), DrawingValidationError> {
    for (index, anchor) in spec.anchors.iter().copied().enumerate() {
        if !predicate(anchor) {
            return Err(DrawingValidationError::UnsupportedAnchor {
                kind: spec.kind,
                anchor_index: index,
            });
        }
    }
    Ok(())
}

fn validate_anchor(anchor: DrawingAnchor, index: usize) -> Result<(), DrawingValidationError> {
    let finite = match anchor {
        DrawingAnchor::TimePrice { price, .. } => price.is_finite(),
        DrawingAnchor::TimeOnly { .. } => true,
        DrawingAnchor::PriceOnly { price } => price.is_finite(),
        DrawingAnchor::LogicalPrice { logical, price } => logical.is_finite() && price.is_finite(),
        DrawingAnchor::Percent { x, y } => {
            if !x.is_finite() || !y.is_finite() {
                return Err(DrawingValidationError::NonFiniteAnchor {
                    anchor_index: index,
                });
            }
            if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
                return Err(DrawingValidationError::PercentAnchorOutOfRange {
                    anchor_index: index,
                });
            }
            true
        }
        DrawingAnchor::StickedOhlc { price, .. } => price.is_finite(),
    };

    if finite {
        Ok(())
    } else {
        Err(DrawingValidationError::NonFiniteAnchor {
            anchor_index: index,
        })
    }
}

fn has_price_coordinate(anchor: DrawingAnchor) -> bool {
    matches!(
        anchor,
        DrawingAnchor::TimePrice { .. }
            | DrawingAnchor::PriceOnly { .. }
            | DrawingAnchor::LogicalPrice { .. }
            | DrawingAnchor::StickedOhlc { .. }
    )
}

fn has_time_coordinate(anchor: DrawingAnchor) -> bool {
    matches!(
        anchor,
        DrawingAnchor::TimePrice { .. }
            | DrawingAnchor::TimeOnly { .. }
            | DrawingAnchor::LogicalPrice { .. }
            | DrawingAnchor::StickedOhlc { .. }
    )
}

fn is_model_point(anchor: DrawingAnchor) -> bool {
    matches!(
        anchor,
        DrawingAnchor::TimePrice { .. }
            | DrawingAnchor::LogicalPrice { .. }
            | DrawingAnchor::StickedOhlc { .. }
    )
}

fn is_model_or_price_point(anchor: DrawingAnchor) -> bool {
    is_model_point(anchor) || matches!(anchor, DrawingAnchor::PriceOnly { .. })
}

fn is_percent_anchor(anchor: DrawingAnchor) -> bool {
    matches!(anchor, DrawingAnchor::Percent { .. })
}

#[cfg(test)]
mod tests {
    use super::{
        DRAWING_SCHEMA_VERSION, DrawingAnchor, DrawingBulkAction, DrawingBulkActionStatus,
        DrawingEditError, DrawingGroupId, DrawingGroupSpec, DrawingId, DrawingKind,
        DrawingLineStyle, DrawingPayload, DrawingSpec, DrawingStore, DrawingValidationError,
        FibRetracementLevel, FibRetracementSettings, PositionDrawingSettings,
    };
    use crate::chart_core::PaneId;

    #[test]
    fn drawing_spec_validates_anchor_counts_by_kind() {
        let trend = DrawingSpec::new(
            DrawingId::new(1),
            DrawingKind::TrendLine,
            PaneId::Main,
            vec![
                DrawingAnchor::TimePrice {
                    time: 100,
                    price: 10.0,
                },
                DrawingAnchor::LogicalPrice {
                    logical: 3.5,
                    price: 12.0,
                },
            ],
        );
        assert!(trend.validate().is_ok());

        let bad = DrawingSpec::new(
            DrawingId::new(2),
            DrawingKind::TrendLine,
            PaneId::Main,
            vec![DrawingAnchor::TimePrice {
                time: 100,
                price: 10.0,
            }],
        );
        assert_eq!(
            bad.validate(),
            Err(DrawingValidationError::WrongAnchorCount {
                kind: DrawingKind::TrendLine,
                expected: "2",
                actual: 1,
            })
        );
    }

    #[test]
    fn line_tools_accept_their_model_space_anchor_types() {
        assert!(
            DrawingSpec::new(
                DrawingId::new(1),
                DrawingKind::HorizontalLine,
                PaneId::Main,
                vec![DrawingAnchor::PriceOnly { price: 42.0 }],
            )
            .validate()
            .is_ok()
        );
        assert!(
            DrawingSpec::new(
                DrawingId::new(2),
                DrawingKind::HorizontalLine,
                PaneId::Main,
                vec![DrawingAnchor::TimePrice {
                    time: 10,
                    price: 42.0,
                }],
            )
            .validate()
            .is_ok()
        );
        assert!(
            DrawingSpec::new(
                DrawingId::new(3),
                DrawingKind::HorizontalRay,
                PaneId::Main,
                vec![DrawingAnchor::TimePrice {
                    time: 10,
                    price: 42.0,
                }],
            )
            .validate()
            .is_ok()
        );
        assert_eq!(
            DrawingSpec::new(
                DrawingId::new(4),
                DrawingKind::HorizontalRay,
                PaneId::Main,
                vec![DrawingAnchor::PriceOnly { price: 42.0 }],
            )
            .validate(),
            Err(DrawingValidationError::UnsupportedAnchor {
                kind: DrawingKind::HorizontalRay,
                anchor_index: 0,
            })
        );
        assert!(
            DrawingSpec::new(
                DrawingId::new(5),
                DrawingKind::VerticalLine,
                PaneId::Main,
                vec![DrawingAnchor::TimeOnly { time: 10 }],
            )
            .validate()
            .is_ok()
        );
        assert!(
            DrawingSpec::new(
                DrawingId::new(5),
                DrawingKind::VerticalLine,
                PaneId::Main,
                vec![DrawingAnchor::LogicalPrice {
                    logical: -3.0,
                    price: 42.0,
                }],
            )
            .validate()
            .is_ok()
        );
        assert_eq!(
            DrawingSpec::new(
                DrawingId::new(4),
                DrawingKind::VerticalLine,
                PaneId::Main,
                vec![DrawingAnchor::PriceOnly { price: 42.0 }],
            )
            .validate(),
            Err(DrawingValidationError::UnsupportedAnchor {
                kind: DrawingKind::VerticalLine,
                anchor_index: 0,
            })
        );
    }

    #[test]
    fn brush_accepts_two_or_more_model_space_points() {
        let brush = DrawingSpec::new(
            DrawingId::new(1),
            DrawingKind::Brush,
            PaneId::Main,
            vec![
                DrawingAnchor::LogicalPrice {
                    logical: 1.0,
                    price: 10.0,
                },
                DrawingAnchor::TimePrice {
                    time: 100,
                    price: 11.0,
                },
                DrawingAnchor::LogicalPrice {
                    logical: 3.0,
                    price: 12.0,
                },
            ],
        );
        assert!(brush.validate().is_ok());

        let too_short = DrawingSpec::new(
            DrawingId::new(2),
            DrawingKind::Brush,
            PaneId::Main,
            vec![DrawingAnchor::LogicalPrice {
                logical: 1.0,
                price: 10.0,
            }],
        );
        assert_eq!(
            too_short.validate(),
            Err(DrawingValidationError::WrongAnchorCount {
                kind: DrawingKind::Brush,
                expected: "2+",
                actual: 1,
            })
        );

        let bad_anchor = DrawingSpec::new(
            DrawingId::new(3),
            DrawingKind::Brush,
            PaneId::Main,
            vec![
                DrawingAnchor::LogicalPrice {
                    logical: 1.0,
                    price: 10.0,
                },
                DrawingAnchor::PriceOnly { price: 11.0 },
            ],
        );
        assert_eq!(
            bad_anchor.validate(),
            Err(DrawingValidationError::UnsupportedAnchor {
                kind: DrawingKind::Brush,
                anchor_index: 1,
            })
        );
    }

    #[test]
    fn fib_retracement_uses_two_model_space_points_and_payload() {
        let fib = DrawingSpec::new(
            DrawingId::new(1),
            DrawingKind::FibRetracement,
            PaneId::Main,
            vec![
                DrawingAnchor::LogicalPrice {
                    logical: 1.0,
                    price: 100.0,
                },
                DrawingAnchor::TimePrice {
                    time: 100,
                    price: 110.0,
                },
            ],
        );
        assert!(fib.validate().is_ok());
        assert!(matches!(
            &fib.payload,
            Some(DrawingPayload::FibRetracement(_))
        ));
        assert!(fib.fib_retracement_settings().levels.len() >= 7);

        let bad_anchor = DrawingSpec::new(
            DrawingId::new(2),
            DrawingKind::FibRetracement,
            PaneId::Main,
            vec![
                DrawingAnchor::PriceOnly { price: 100.0 },
                DrawingAnchor::LogicalPrice {
                    logical: 2.0,
                    price: 110.0,
                },
            ],
        );
        assert_eq!(
            bad_anchor.validate(),
            Err(DrawingValidationError::UnsupportedAnchor {
                kind: DrawingKind::FibRetracement,
                anchor_index: 0,
            })
        );

        let invalid_payload = DrawingSpec::new(
            DrawingId::new(3),
            DrawingKind::FibRetracement,
            PaneId::Main,
            vec![
                DrawingAnchor::LogicalPrice {
                    logical: 1.0,
                    price: 100.0,
                },
                DrawingAnchor::LogicalPrice {
                    logical: 2.0,
                    price: 110.0,
                },
            ],
        )
        .with_payload(DrawingPayload::FibRetracement(FibRetracementSettings {
            levels: vec![FibRetracementLevel::new(
                f64::NAN,
                crate::chart_core::ChartColor::rgb(255, 0, 0),
            )],
            ..FibRetracementSettings::default()
        }));
        assert_eq!(
            invalid_payload.validate(),
            Err(DrawingValidationError::InvalidFibLevel { level_index: 0 })
        );
    }

    #[test]
    fn position_drawings_use_three_directional_model_space_anchors_and_payload() {
        let long = DrawingSpec::new(
            DrawingId::new(1),
            DrawingKind::LongPosition,
            PaneId::Main,
            vec![
                DrawingAnchor::LogicalPrice {
                    logical: 1.0,
                    price: 100.0,
                },
                DrawingAnchor::LogicalPrice {
                    logical: 8.0,
                    price: 112.0,
                },
                DrawingAnchor::LogicalPrice {
                    logical: 8.0,
                    price: 94.0,
                },
            ],
        );
        assert!(long.validate().is_ok());
        assert!(matches!(&long.payload, Some(DrawingPayload::Position(_))));
        assert_eq!(long.position_settings(), PositionDrawingSettings::default());

        let short = DrawingSpec::new(
            DrawingId::new(2),
            DrawingKind::ShortPosition,
            PaneId::Main,
            vec![
                DrawingAnchor::LogicalPrice {
                    logical: 1.0,
                    price: 100.0,
                },
                DrawingAnchor::LogicalPrice {
                    logical: 8.0,
                    price: 88.0,
                },
                DrawingAnchor::LogicalPrice {
                    logical: 8.0,
                    price: 106.0,
                },
            ],
        );
        assert!(short.validate().is_ok());

        let wrong_direction = DrawingSpec::new(
            DrawingId::new(3),
            DrawingKind::LongPosition,
            PaneId::Main,
            vec![
                DrawingAnchor::LogicalPrice {
                    logical: 1.0,
                    price: 100.0,
                },
                DrawingAnchor::LogicalPrice {
                    logical: 8.0,
                    price: 96.0,
                },
                DrawingAnchor::LogicalPrice {
                    logical: 8.0,
                    price: 92.0,
                },
            ],
        );
        assert_eq!(
            wrong_direction.validate(),
            Err(DrawingValidationError::InvalidPositionAnchors {
                kind: DrawingKind::LongPosition
            })
        );

        let bad_anchor = DrawingSpec::new(
            DrawingId::new(4),
            DrawingKind::ShortPosition,
            PaneId::Main,
            vec![
                DrawingAnchor::PriceOnly { price: 100.0 },
                DrawingAnchor::LogicalPrice {
                    logical: 8.0,
                    price: 88.0,
                },
                DrawingAnchor::LogicalPrice {
                    logical: 8.0,
                    price: 106.0,
                },
            ],
        );
        assert_eq!(
            bad_anchor.validate(),
            Err(DrawingValidationError::UnsupportedAnchor {
                kind: DrawingKind::ShortPosition,
                anchor_index: 0,
            })
        );

        let wrong_count = DrawingSpec::new(
            DrawingId::new(5),
            DrawingKind::LongPosition,
            PaneId::Main,
            vec![
                DrawingAnchor::LogicalPrice {
                    logical: 1.0,
                    price: 100.0,
                },
                DrawingAnchor::LogicalPrice {
                    logical: 8.0,
                    price: 112.0,
                },
            ],
        );
        assert_eq!(
            wrong_count.validate(),
            Err(DrawingValidationError::WrongAnchorCount {
                kind: DrawingKind::LongPosition,
                expected: "3",
                actual: 2,
            })
        );
    }

    #[test]
    fn anchored_text_uses_percent_coordinates() {
        let anchored = DrawingSpec::new(
            DrawingId::new(1),
            DrawingKind::AnchoredText,
            PaneId::Main,
            vec![DrawingAnchor::Percent { x: 0.15, y: 0.75 }],
        );
        assert!(anchored.validate().is_ok());

        let out_of_range = DrawingSpec::new(
            DrawingId::new(2),
            DrawingKind::AnchoredText,
            PaneId::Main,
            vec![DrawingAnchor::Percent { x: 1.15, y: 0.75 }],
        );
        assert_eq!(
            out_of_range.validate(),
            Err(DrawingValidationError::PercentAnchorOutOfRange { anchor_index: 0 })
        );
    }

    #[test]
    fn store_assigns_stable_monotonic_ids_and_visible_iterators_keep_hidden_state() {
        let mut store = DrawingStore::new();

        assert_eq!(
            store.create(
                DrawingKind::VerticalLine,
                PaneId::Main,
                vec![DrawingAnchor::PriceOnly { price: 99.0 }],
            ),
            Err(DrawingValidationError::UnsupportedAnchor {
                kind: DrawingKind::VerticalLine,
                anchor_index: 0,
            })
        );

        let first = store
            .create(
                DrawingKind::HorizontalLine,
                PaneId::Main,
                vec![DrawingAnchor::PriceOnly { price: 10.0 }],
            )
            .expect("first drawing");
        let second = store
            .create(
                DrawingKind::VerticalLine,
                PaneId::Main,
                vec![DrawingAnchor::TimeOnly { time: 20 }],
            )
            .expect("second drawing");

        assert_eq!(first, DrawingId::new(1));
        assert_eq!(second, DrawingId::new(2));
        assert!(store.set_visible(first, false));
        assert_eq!(
            store
                .visible()
                .map(|drawing| drawing.id)
                .collect::<Vec<_>>(),
            vec![second]
        );
        assert_eq!(store.drawings().len(), 2);

        store
            .remove(first)
            .expect("hidden drawing remains removable");
        let third = store
            .create(
                DrawingKind::VerticalLine,
                PaneId::Main,
                vec![DrawingAnchor::TimeOnly { time: 30 }],
            )
            .expect("third drawing");
        assert_eq!(third, DrawingId::new(3));
    }

    #[test]
    fn locked_drawings_reject_move_style_text_payload_and_delete_mutations() {
        let mut store = DrawingStore::new();
        let id = store
            .insert(
                DrawingSpec::new(
                    DrawingId::new(10),
                    DrawingKind::HorizontalLine,
                    PaneId::Main,
                    vec![DrawingAnchor::PriceOnly { price: 10.0 }],
                )
                .with_locked(true),
            )
            .expect("locked drawing");

        assert_eq!(
            store.replace_anchors(id, vec![DrawingAnchor::PriceOnly { price: 11.0 }]),
            Err(DrawingEditError::LockedDrawing(id))
        );
        let mut style = store.get(id).expect("drawing exists").style.clone();
        style.line_style = DrawingLineStyle::Dashed;
        assert_eq!(
            store.replace_style(id, style),
            Err(DrawingEditError::LockedDrawing(id))
        );
        assert_eq!(
            store.set_text(id, Some("Locked".to_string())),
            Err(DrawingEditError::LockedDrawing(id))
        );
        assert_eq!(
            store.replace_payload(
                id,
                Some(DrawingPayload::FibRetracement(
                    FibRetracementSettings::default(),
                )),
            ),
            Err(DrawingEditError::LockedDrawing(id))
        );
        assert_eq!(store.remove(id), Err(DrawingEditError::LockedDrawing(id)));
        assert!(store.get(id).is_some());
    }

    #[test]
    fn replace_payload_validates_kind_specific_settings() {
        let mut store = DrawingStore::new();
        let id = store
            .create(
                DrawingKind::FibRetracement,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 1.0,
                        price: 100.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 2.0,
                        price: 110.0,
                    },
                ],
            )
            .expect("fib drawing");
        let payload = DrawingPayload::FibRetracement(FibRetracementSettings {
            levels: vec![FibRetracementLevel::new(
                1.272,
                crate::chart_core::ChartColor::rgb(255, 152, 0),
            )],
            ..FibRetracementSettings::default()
        });

        store
            .replace_payload(id, Some(payload.clone()))
            .expect("replace payload");
        assert_eq!(
            store.get(id).and_then(|drawing| drawing.payload.clone()),
            Some(payload)
        );

        assert_eq!(
            store.replace_payload(
                id,
                Some(DrawingPayload::FibRetracement(FibRetracementSettings {
                    levels: vec![FibRetracementLevel::new(
                        f64::INFINITY,
                        crate::chart_core::ChartColor::rgb(255, 0, 0),
                    )],
                    ..FibRetracementSettings::default()
                })),
            ),
            Err(DrawingEditError::Invalid(
                DrawingValidationError::InvalidFibLevel { level_index: 0 }
            ))
        );

        assert_eq!(
            store.replace_payload(DrawingId::new(404), None),
            Err(DrawingEditError::MissingDrawing(DrawingId::new(404)))
        );
    }

    #[test]
    fn drawing_style_replacement_validates_and_bumps_generation() {
        let mut store = DrawingStore::new();
        let id = store
            .insert(DrawingSpec::new(
                DrawingId::new(1),
                DrawingKind::HorizontalLine,
                PaneId::Main,
                vec![DrawingAnchor::PriceOnly { price: 10.0 }],
            ))
            .expect("horizontal line should insert");
        let generation = store.generation();
        let mut style = store.get(id).expect("drawing exists").style.clone();
        style.line_style = DrawingLineStyle::Dashed;
        style.stroke_width = 3.0;

        store
            .replace_style(id, style.clone())
            .expect("valid style should update");

        assert_eq!(store.get(id).expect("drawing exists").style, style);
        assert!(store.generation() > generation);
    }

    #[test]
    fn drawing_text_replacement_tracks_generation() {
        let mut store = DrawingStore::new();
        let id = store
            .insert(DrawingSpec::new(
                DrawingId::new(1),
                DrawingKind::Text,
                PaneId::Main,
                vec![DrawingAnchor::LogicalPrice {
                    logical: 1.0,
                    price: 10.0,
                }],
            ))
            .expect("text should insert");
        let generation = store.generation();

        store
            .set_text(id, Some("Plan".to_string()))
            .expect("text update should work");

        assert_eq!(
            store.get(id).expect("drawing exists").text.as_deref(),
            Some("Plan")
        );
        assert!(store.generation() > generation);
    }

    #[test]
    fn restore_from_restores_snapshot_and_advances_generation() {
        let mut store = DrawingStore::new();
        let id = store
            .create(
                DrawingKind::HorizontalLine,
                PaneId::Main,
                vec![DrawingAnchor::PriceOnly { price: 10.0 }],
            )
            .expect("drawing");
        let snapshot = store.clone();
        let generation_before_change = store.generation();

        store
            .replace_anchors(id, vec![DrawingAnchor::PriceOnly { price: 11.0 }])
            .expect("move drawing");
        let generation_before_restore = store.generation();
        store.restore_from(snapshot);

        assert_eq!(
            store.get(id).expect("drawing").anchors,
            vec![DrawingAnchor::PriceOnly { price: 10.0 }]
        );
        assert!(store.generation() > generation_before_change);
        assert!(store.generation() > generation_before_restore);
    }

    #[test]
    fn group_validation_rejects_cross_pane_and_multi_group_membership() {
        let mut store = DrawingStore::new();
        let main = store
            .create(
                DrawingKind::HorizontalLine,
                PaneId::Main,
                vec![DrawingAnchor::PriceOnly { price: 10.0 }],
            )
            .expect("main drawing");
        let other_main = store
            .create(
                DrawingKind::VerticalLine,
                PaneId::Main,
                vec![DrawingAnchor::TimeOnly { time: 20 }],
            )
            .expect("other main drawing");
        let study = store
            .create(
                DrawingKind::HorizontalLine,
                PaneId::Study(1),
                vec![DrawingAnchor::PriceOnly { price: 30.0 }],
            )
            .expect("study drawing");

        assert_eq!(
            store.create_group(vec![main, study]),
            Err(DrawingValidationError::CrossPaneGroup)
        );
        let group = store
            .create_group(vec![main, other_main])
            .expect("same pane group");
        assert_eq!(group, DrawingGroupId::new(1));
        assert_eq!(
            store.create_group(vec![main]),
            Err(DrawingValidationError::DrawingAlreadyGrouped(main))
        );
    }

    #[test]
    fn persisted_parts_restore_drawings_groups_and_next_ids() {
        let first = DrawingId::new(7);
        let second = DrawingId::new(8);
        let group = DrawingGroupId::new(4);
        let drawings = vec![
            DrawingSpec::new(
                first,
                DrawingKind::Rectangle,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 1.0,
                        price: 10.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 2.0,
                        price: 12.0,
                    },
                ],
            )
            .with_symbol("YFinance:AAPL")
            .with_z_order(2),
            DrawingSpec::new(
                second,
                DrawingKind::HorizontalLine,
                PaneId::Main,
                vec![DrawingAnchor::PriceOnly { price: 11.0 }],
            )
            .with_symbol("YFinance:AAPL")
            .with_z_order(3),
        ];
        let groups = vec![DrawingGroupSpec {
            id: group,
            pane: PaneId::Main,
            symbol: Some("YFinance:AAPL".to_string()),
            drawing_ids: vec![first, second],
        }];

        let store =
            DrawingStore::from_persisted_parts(drawings, groups, 12, 9).expect("persisted store");

        assert_eq!(store.next_drawing_id(), 12);
        assert_eq!(store.next_group_id(), 9);
        assert_eq!(store.groups().len(), 1);
        assert_eq!(store.get(first).expect("first").group_id, Some(group));
        assert_eq!(store.get(second).expect("second").group_id, Some(group));
        assert_eq!(store.generation(), 0);
    }

    #[test]
    fn persisted_parts_reject_dangling_group_membership() {
        let drawing = DrawingSpec::new(
            DrawingId::new(7),
            DrawingKind::HorizontalLine,
            PaneId::Main,
            vec![DrawingAnchor::PriceOnly { price: 11.0 }],
        )
        .with_symbol("YFinance:AAPL");
        let mut grouped = drawing;
        grouped.group_id = Some(DrawingGroupId::new(99));

        assert_eq!(
            DrawingStore::from_persisted_parts(vec![grouped], Vec::new(), 8, 1),
            Err(DrawingValidationError::MissingGroup(DrawingGroupId::new(
                99
            )))
        );
    }

    #[test]
    fn z_order_apis_move_and_normalize_drawings_deterministically() {
        let mut store = DrawingStore::new();
        let first = store
            .insert(
                DrawingSpec::new(
                    DrawingId::new(10),
                    DrawingKind::HorizontalLine,
                    PaneId::Main,
                    vec![DrawingAnchor::PriceOnly { price: 10.0 }],
                )
                .with_z_order(5),
            )
            .expect("first");
        let second = store
            .insert(
                DrawingSpec::new(
                    DrawingId::new(11),
                    DrawingKind::VerticalLine,
                    PaneId::Main,
                    vec![DrawingAnchor::TimeOnly { time: 20 }],
                )
                .with_z_order(5),
            )
            .expect("second");
        let third = store
            .insert(
                DrawingSpec::new(
                    DrawingId::new(12),
                    DrawingKind::Text,
                    PaneId::Main,
                    vec![DrawingAnchor::LogicalPrice {
                        logical: 1.0,
                        price: 12.0,
                    }],
                )
                .with_z_order(20),
            )
            .expect("third");

        assert!(store.normalize_z_order());
        assert_eq!(
            store
                .drawings()
                .iter()
                .map(|drawing| (drawing.id, drawing.z_order))
                .collect::<Vec<_>>(),
            vec![(first, 0), (second, 1), (third, 2)]
        );

        assert!(store.bring_forward(first).expect("move forward"));
        assert_eq!(
            store
                .drawings()
                .iter()
                .map(|drawing| drawing.id)
                .collect::<Vec<_>>(),
            vec![second, first, third]
        );
        assert!(store.bring_to_front(second).expect("front"));
        assert_eq!(
            store
                .drawings()
                .iter()
                .map(|drawing| drawing.id)
                .collect::<Vec<_>>(),
            vec![first, third, second]
        );
        assert!(store.send_backward(second).expect("back one"));
        assert_eq!(
            store
                .drawings()
                .iter()
                .map(|drawing| drawing.id)
                .collect::<Vec<_>>(),
            vec![first, second, third]
        );
        assert!(store.send_to_back(third).expect("send to back"));
        assert_eq!(
            store
                .drawings()
                .iter()
                .map(|drawing| drawing.id)
                .collect::<Vec<_>>(),
            vec![third, first, second]
        );
        assert!(!store.send_to_back(third).expect("already back"));
    }

    #[test]
    fn group_ungroup_and_member_removal_preserve_membership_invariants() {
        let mut store = DrawingStore::new();
        let first = store
            .insert(
                DrawingSpec::new(
                    DrawingId::new(1),
                    DrawingKind::HorizontalLine,
                    PaneId::Main,
                    vec![DrawingAnchor::PriceOnly { price: 10.0 }],
                )
                .with_symbol("NASDAQ:AAPL"),
            )
            .expect("first");
        let second = store
            .insert(
                DrawingSpec::new(
                    DrawingId::new(2),
                    DrawingKind::VerticalLine,
                    PaneId::Main,
                    vec![DrawingAnchor::TimeOnly { time: 20 }],
                )
                .with_symbol("NASDAQ:AAPL"),
            )
            .expect("second");
        let third = store
            .insert(
                DrawingSpec::new(
                    DrawingId::new(3),
                    DrawingKind::Text,
                    PaneId::Main,
                    vec![DrawingAnchor::LogicalPrice {
                        logical: 1.0,
                        price: 12.0,
                    }],
                )
                .with_symbol("NASDAQ:AAPL"),
            )
            .expect("third");
        let other_symbol = store
            .insert(
                DrawingSpec::new(
                    DrawingId::new(4),
                    DrawingKind::HorizontalLine,
                    PaneId::Main,
                    vec![DrawingAnchor::PriceOnly { price: 20.0 }],
                )
                .with_symbol("NASDAQ:MSFT"),
            )
            .expect("other symbol");

        assert_eq!(
            store.create_group(vec![first, other_symbol]),
            Err(DrawingValidationError::CrossSymbolGroup)
        );
        assert_eq!(
            store.create_group(vec![first]),
            Err(DrawingValidationError::TooFewGroupMembers)
        );
        let group = store
            .create_group(vec![first, second, third])
            .expect("same symbol group");
        assert_eq!(store.groups().len(), 1);
        assert_eq!(store.get(first).expect("first").group_id, Some(group));

        assert_eq!(
            store.remove_from_group(first).expect("remove member"),
            Some(group)
        );
        assert_eq!(store.groups()[0].drawing_ids, vec![second, third]);
        assert_eq!(store.get(first).expect("first").group_id, None);

        store.remove(second).expect("delete grouped member");
        assert!(store.groups().is_empty());
        assert_eq!(store.get(third).expect("third").group_id, None);
    }

    #[test]
    fn ungroup_clears_group_memberships() {
        let mut store = DrawingStore::new();
        let first = store
            .create(
                DrawingKind::HorizontalLine,
                PaneId::Main,
                vec![DrawingAnchor::PriceOnly { price: 10.0 }],
            )
            .expect("first");
        let second = store
            .create(
                DrawingKind::VerticalLine,
                PaneId::Main,
                vec![DrawingAnchor::TimeOnly { time: 20 }],
            )
            .expect("second");
        let group = store.create_group(vec![first, second]).expect("group");

        assert_eq!(store.ungroup(group), Ok(vec![first, second]));
        assert!(store.groups().is_empty());
        assert_eq!(store.get(first).expect("first").group_id, None);
        assert_eq!(store.get(second).expect("second").group_id, None);
        assert_eq!(
            store.ungroup(group),
            Err(DrawingEditError::MissingGroup(group))
        );
    }

    #[test]
    fn bulk_actions_report_locked_missing_and_unchanged_outcomes() {
        let mut store = DrawingStore::new();
        let editable = store
            .create(
                DrawingKind::HorizontalLine,
                PaneId::Main,
                vec![DrawingAnchor::PriceOnly { price: 10.0 }],
            )
            .expect("editable");
        let locked = store
            .insert(
                DrawingSpec::new(
                    DrawingId::new(8),
                    DrawingKind::VerticalLine,
                    PaneId::Main,
                    vec![DrawingAnchor::TimeOnly { time: 20 }],
                )
                .with_locked(true),
            )
            .expect("locked");

        let visibility = store.apply_bulk_action(
            &[editable, locked, DrawingId::new(404)],
            DrawingBulkAction::SetVisible(false),
        );
        assert_eq!(
            visibility
                .iter()
                .map(|outcome| outcome.status.clone())
                .collect::<Vec<_>>(),
            vec![
                DrawingBulkActionStatus::Changed,
                DrawingBulkActionStatus::Changed,
                DrawingBulkActionStatus::Error(DrawingEditError::MissingDrawing(DrawingId::new(
                    404
                ))),
            ]
        );
        assert_eq!(
            store.apply_bulk_action(&[editable], DrawingBulkAction::SetVisible(false))[0].status,
            DrawingBulkActionStatus::Unchanged
        );
        assert_eq!(
            store.apply_bulk_action(&[editable], DrawingBulkAction::SetSelectable(false))[0].status,
            DrawingBulkActionStatus::Changed
        );

        let delete = store.apply_bulk_action(&[editable, locked], DrawingBulkAction::Delete);
        assert_eq!(delete[0].status, DrawingBulkActionStatus::Deleted);
        assert_eq!(
            delete[1].status,
            DrawingBulkActionStatus::Error(DrawingEditError::LockedDrawing(locked))
        );
        assert!(store.get(editable).is_none());
        assert!(store.get(locked).is_some());
    }

    #[test]
    fn inserted_drawings_validate_ids_schema_and_duplicates() {
        let mut store = DrawingStore::new();
        let spec = DrawingSpec::new(
            DrawingId::new(5),
            DrawingKind::Text,
            PaneId::Main,
            vec![DrawingAnchor::TimePrice {
                time: 100,
                price: 10.0,
            }],
        );
        assert_eq!(store.insert(spec.clone()), Ok(DrawingId::new(5)));
        assert_eq!(
            store.insert(spec),
            Err(DrawingValidationError::DuplicateDrawingId(DrawingId::new(
                5
            )))
        );

        let mut stale_schema = DrawingSpec::new(
            DrawingId::new(6),
            DrawingKind::Text,
            PaneId::Main,
            vec![DrawingAnchor::TimePrice {
                time: 100,
                price: 10.0,
            }],
        );
        stale_schema.schema_version = DRAWING_SCHEMA_VERSION + 1;
        assert_eq!(
            stale_schema.validate(),
            Err(DrawingValidationError::UnsupportedSchemaVersion(
                DRAWING_SCHEMA_VERSION + 1
            ))
        );
    }

    #[test]
    fn insert_copy_assigns_fresh_ids_and_clears_group_membership() {
        let mut store = DrawingStore::new();
        let first = store
            .insert(DrawingSpec::new(
                DrawingId::new(10),
                DrawingKind::HorizontalLine,
                PaneId::Main,
                vec![DrawingAnchor::PriceOnly { price: 10.0 }],
            ))
            .expect("first drawing");
        let second = store
            .insert(DrawingSpec::new(
                DrawingId::new(11),
                DrawingKind::VerticalLine,
                PaneId::Main,
                vec![DrawingAnchor::TimeOnly { time: 20 }],
            ))
            .expect("second drawing");
        store
            .create_group(vec![first, second])
            .expect("same-pane group");

        let source = store.get(first).expect("source drawing").clone();
        let copy = store.insert_copy(source).expect("copied drawing");

        assert_eq!(copy, DrawingId::new(12));
        let copied = store.get(copy).expect("copy exists");
        assert_eq!(copied.group_id, None);
        assert_eq!(copied.kind, DrawingKind::HorizontalLine);
        assert_eq!(store.groups().len(), 1);
    }

    #[test]
    fn insert_copy_preserves_brush_geometry_with_fresh_id() {
        let mut store = DrawingStore::new();
        let brush = store
            .insert(DrawingSpec::new(
                DrawingId::new(20),
                DrawingKind::Brush,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 1.0,
                        price: 10.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 1.5,
                        price: 10.8,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 2.0,
                        price: 10.2,
                    },
                ],
            ))
            .expect("brush drawing");

        let copy = store
            .insert_copy(store.get(brush).expect("source brush").clone())
            .expect("brush copy");

        assert_eq!(copy, DrawingId::new(21));
        assert_eq!(
            store.get(copy).expect("copied brush").anchors,
            store.get(brush).expect("source brush").anchors
        );
        assert_eq!(
            store.get(copy).expect("copied brush").kind,
            DrawingKind::Brush
        );
    }
}
