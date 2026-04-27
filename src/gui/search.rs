use fuzzy_matcher::FuzzyMatcher;

use crate::{
    resource::manifest::Manifest,
    scan::{
        game_filter::{self, FilterKind},
        Duplication, ScanInfo,
    },
};

#[derive(Default, Clone, Eq, PartialEq)]
pub struct Filter<T> {
    active: bool,
    pub choice: T,
}

#[derive(Default)]
pub struct FilterComponent {
    pub show: bool,
    pub game_name: String,
    pub uniqueness: Filter<game_filter::Uniqueness>,
    pub completeness: Filter<game_filter::Completeness>,
    pub enablement: Filter<game_filter::Enablement>,
    pub change: Filter<game_filter::Change>,
    pub manifest: Filter<game_filter::Manifest>,
}

impl FilterComponent {
    pub fn reset(&mut self) {
        self.game_name.clear();
        self.uniqueness.active = false;
        self.completeness.active = false;
        self.enablement.active = false;
        self.change.active = false;
        self.manifest.active = false;
    }

    pub fn qualifies(
        &self,
        scan: &ScanInfo,
        manifest: &Manifest,
        enabled: bool,
        customized: bool,
        duplicated: Duplication,
        show_deselected_games: bool,
    ) -> bool {
        if !self.show {
            return true;
        }

        let fuzzy = self.game_name.is_empty()
            || fuzzy_matcher::skim::SkimMatcherV2::default()
                .fuzzy_match(&scan.game_name.to_lowercase(), &self.game_name.to_lowercase())
                .is_some();
        let unique = !self.uniqueness.active || self.uniqueness.choice.qualifies(duplicated);
        let complete = !self.completeness.active || self.completeness.choice.qualifies(scan);
        let enable = !show_deselected_games || !self.enablement.active || self.enablement.choice.qualifies(enabled);
        let changed = !self.change.active || self.change.choice.qualifies(scan);
        let manifest = !self.manifest.active
            || self
                .manifest
                .choice
                .qualifies(manifest.0.get(&scan.game_name), customized);

        fuzzy && unique && complete && changed && enable && manifest
    }

    pub fn toggle_filter(&mut self, filter: FilterKind, enabled: bool) {
        match filter {
            FilterKind::Uniqueness => self.uniqueness.active = enabled,
            FilterKind::Completeness => self.completeness.active = enabled,
            FilterKind::Enablement => self.enablement.active = enabled,
            FilterKind::Change => self.change.active = enabled,
            FilterKind::Manifest => self.manifest.active = enabled,
        }
    }
}

#[derive(Default)]
pub struct CustomGamesFilter {
    pub enabled: bool,
    pub name: String,
}

impl CustomGamesFilter {
    pub fn reset(&mut self) {
        self.name.clear();
    }
}
