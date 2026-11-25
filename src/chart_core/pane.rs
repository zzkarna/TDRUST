#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum PaneId {
    Main,
    Volume,
    Study(u32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaneSpec {
    pub id: PaneId,
    pub weight: f32,
    pub min_height: f32,
    pub visible: bool,
}

impl PaneSpec {
    pub const fn new(id: PaneId, weight: f32, min_height: f32, visible: bool) -> Self {
        Self {
            id,
            weight,
            min_height,
            visible,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaneRect<T> {
    pub id: PaneId,
    pub rect: T,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PaneModel {
    panes: Vec<PaneSpec>,
}

impl Default for PaneModel {
    fn default() -> Self {
        Self {
            panes: vec![main_pane_spec(), volume_pane_spec()],
        }
    }
}

impl PaneModel {
    pub fn new(panes: Vec<PaneSpec>) -> Self {
        Self { panes }
    }

    pub fn panes(&self) -> &[PaneSpec] {
        &self.panes
    }

    pub fn pane(&self, id: PaneId) -> Option<PaneSpec> {
        self.panes.iter().copied().find(|pane| pane.id == id)
    }

    pub fn is_visible(&self, id: PaneId) -> bool {
        self.pane(id).is_some_and(|pane| pane.visible)
    }

    pub fn visible_panes(&self) -> impl Iterator<Item = PaneSpec> + '_ {
        self.panes.iter().copied().filter(|pane| pane.visible)
    }

    pub fn set_visible(&mut self, id: PaneId, visible: bool) {
        if let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == id) {
            pane.visible = visible;
        }
    }

    pub fn set_weight(&mut self, id: PaneId, weight: f32) {
        if let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == id) {
            pane.weight = weight.max(0.0);
        }
    }

    pub fn add_or_replace(&mut self, spec: PaneSpec) {
        if let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == spec.id) {
            *pane = spec;
        } else {
            self.panes.push(spec);
        }
    }

    pub fn normalized_visible_weights(&self) -> Vec<(PaneId, f32, f32)> {
        let visible = self.visible_panes().collect::<Vec<_>>();
        let total = visible
            .iter()
            .map(|pane| pane.weight.max(0.0))
            .sum::<f32>()
            .max(f32::EPSILON);

        visible
            .into_iter()
            .map(|pane| (pane.id, pane.weight.max(0.0) / total, pane.min_height))
            .collect()
    }
}

pub const fn main_pane_spec() -> PaneSpec {
    PaneSpec::new(PaneId::Main, 1.0, 180.0, true)
}

pub const fn volume_pane_spec() -> PaneSpec {
    PaneSpec::new(PaneId::Volume, 0.18, 54.0, true)
}

#[cfg(test)]
mod tests {
    use super::{PaneId, PaneModel, PaneSpec};

    #[test]
    fn default_model_has_main_and_volume_panes() {
        let panes = PaneModel::default();

        assert_eq!(panes.panes().len(), 2);
        assert_eq!(
            panes
                .visible_panes()
                .map(|pane| pane.id)
                .collect::<Vec<_>>(),
            vec![PaneId::Main, PaneId::Volume]
        );
    }

    #[test]
    fn pane_visibility_controls_visible_set() {
        let mut panes = PaneModel::default();

        panes.set_visible(PaneId::Volume, false);

        assert!(!panes.is_visible(PaneId::Volume));
        assert_eq!(
            panes
                .visible_panes()
                .map(|pane| pane.id)
                .collect::<Vec<_>>(),
            vec![PaneId::Main]
        );
    }

    #[test]
    fn normalized_weights_ignore_hidden_panes() {
        let panes = PaneModel::new(vec![
            PaneSpec::new(PaneId::Main, 3.0, 180.0, true),
            PaneSpec::new(PaneId::Volume, 1.0, 54.0, true),
            PaneSpec::new(PaneId::Study(1), 10.0, 80.0, false),
        ]);
        let weights = panes.normalized_visible_weights();

        assert_eq!(weights.len(), 2);
        assert_eq!(weights[0].0, PaneId::Main);
        assert!((weights[0].1 - 0.75).abs() < 1e-6);
        assert_eq!(weights[1].0, PaneId::Volume);
        assert!((weights[1].1 - 0.25).abs() < 1e-6);
    }

    #[test]
    fn pane_weights_can_be_updated_for_resizing() {
        let mut panes = PaneModel::default();

        panes.set_weight(PaneId::Volume, 1.0);

        assert_eq!(panes.pane(PaneId::Volume).unwrap().weight, 1.0);
        let weights = panes.normalized_visible_weights();
        assert!((weights[0].1 - 0.5).abs() < 1e-6);
        assert!((weights[1].1 - 0.5).abs() < 1e-6);
    }

    #[test]
    fn study_panes_attach_to_model_order() {
        let mut panes = PaneModel::default();

        panes.add_or_replace(PaneSpec::new(PaneId::Study(1), 0.35, 80.0, true));

        assert_eq!(
            panes
                .visible_panes()
                .map(|pane| pane.id)
                .collect::<Vec<_>>(),
            vec![PaneId::Main, PaneId::Volume, PaneId::Study(1)]
        );
        assert_eq!(panes.pane(PaneId::Study(1)).unwrap().min_height, 80.0);
    }
}
