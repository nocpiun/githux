//! Profile overview state and host-facing update API on Native Shell.
//!
//! The host supplies already-read public profile data, a prepared same-name
//! repository README document and an explicit validated contribution interval.
//! This module performs no I/O, authorization inference, avatar fetching, README
//! discovery or date defaulting. State lives in one NSH signal created by
//! [`ProfileView::new`] inside the page's reactive root; every update validates
//! the host's request key first, so a late reply can never land on a page that
//! was rebound in the meantime.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::SystemTime;

use nsh_core::element::ImageSrc;
use nsh_core::prelude::*;

use crate::domain::{ContributionCalendar, ContributionRange, ProfileBasic, ProfilePinned};
use crate::ui::reader::images::ImageBridge;
use crate::ui::reader::{ReaderDocument, ReaderHandle, ReaderLayout, reader};

use super::{
    ProfileActionKind, ProfileBinding, ProfileProvenance, ProfileRequest, ProfileTarget,
    SectionState, heatmap::HeatmapPresentation, model::repository_presentation,
};

pub type OnProfileAction = Rc<dyn Fn(super::ProfileAction)>;

/// Everything one binding presents. One signal keeps validity checks and the page
/// repaint in one place; the page is a document, not a high-frequency list.
#[derive(Clone, PartialEq)]
pub struct ProfileUi {
    pub(super) target: ProfileTarget,
    pub(super) binding: ProfileBinding,
    pub(super) generation: u64,
    pub(super) range: ContributionRange,
    pub(super) basic: SectionState<ProfileBasic>,
    pub(super) repositories: SectionState<ProfilePinned>,
    pub(super) contributions: SectionState<ContributionCalendar>,
    pub(super) readme: SectionState<()>,
    pub(super) basic_provenance: Option<ProfileProvenance>,
    pub(super) repositories_provenance: Option<ProfileProvenance>,
    pub(super) contributions_provenance: Option<ProfileProvenance>,
    pub(super) readme_provenance: Option<ProfileProvenance>,
    pub(super) avatar: Option<ImageSrc>,
    pub(super) now: SystemTime,
    pub(super) selected_day: Option<usize>,
    pub(super) readme_identity: Option<String>,
}

impl ProfileUi {
    fn reset(&mut self, target: ProfileTarget, binding: ProfileBinding, range: ContributionRange) {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("profile request generation exhausted");
        self.target = target;
        self.binding = binding;
        self.range = range;
        self.basic = SectionState::Loading;
        self.repositories = SectionState::Loading;
        self.contributions = SectionState::Loading;
        self.readme = SectionState::Loading;
        self.basic_provenance = None;
        self.repositories_provenance = None;
        self.contributions_provenance = None;
        self.readme_provenance = None;
        self.avatar = None;
        self.selected_day = None;
        self.readme_identity = None;
    }

    pub(super) fn request(&self) -> ProfileRequest {
        ProfileRequest {
            target: self.target.clone(),
            binding: self.binding.clone(),
            generation: self.generation,
        }
    }
}

/// The mounted README reader. Not a signal: it is replaced as a unit and its
/// presence is mirrored by [`ProfileUi::readme`].
type ReaderSlot = Rc<RefCell<Option<ReaderHandle>>>;

#[derive(Clone)]
pub struct ProfileView {
    pub(super) ui: Signal<ProfileUi>,
    reader: ReaderSlot,
    on_action: Rc<RefCell<Option<OnProfileAction>>>,
    /// Repaint trigger for reader installation/removal, which is not signal state.
    pub(super) reader_epoch: Signal<u64>,
}

impl ProfileView {
    /// Construct one per page inside the page's reactive root.
    pub fn new(target: ProfileTarget, binding: ProfileBinding, range: ContributionRange) -> Self {
        let reader: ReaderSlot = Rc::new(RefCell::new(None));
        Self {
            ui: signal(ProfileUi {
                target,
                binding,
                generation: 0,
                range,
                basic: SectionState::Loading,
                repositories: SectionState::Loading,
                contributions: SectionState::Loading,
                readme: SectionState::Loading,
                basic_provenance: None,
                repositories_provenance: None,
                contributions_provenance: None,
                readme_provenance: None,
                avatar: None,
                now: SystemTime::now(),
                selected_day: None,
                readme_identity: None,
            }),
            reader,
            on_action: Rc::new(RefCell::new(None)),
            reader_epoch: signal(0),
        }
    }

    /// Begin a new host binding. Every visible section and the mounted reader are
    /// invalidated; results carrying the previous request key are rejected even if
    /// the new profile has the same login.
    pub fn bind(
        &mut self,
        target: ProfileTarget,
        binding: ProfileBinding,
        range: ContributionRange,
    ) -> ProfileRequest {
        self.drop_reader();
        self.ui.update(|ui| ui.reset(target, binding, range));
        self.ui.with(|ui| ui.request())
    }

    pub fn request(&self) -> ProfileRequest {
        self.ui.with(ProfileUi::request)
    }

    pub fn target(&self) -> ProfileTarget {
        self.ui.with(|ui| ui.target.clone())
    }

    pub fn binding(&self) -> ProfileBinding {
        self.ui.with(|ui| ui.binding.clone())
    }

    pub fn range(&self) -> ContributionRange {
        self.ui.with(|ui| ui.range.clone())
    }

    pub fn accepts(&self, request: &ProfileRequest) -> bool {
        *request == self.request()
    }

    pub fn basic(&self) -> SectionState<ProfileBasic> {
        self.ui.with(|ui| ui.basic.clone())
    }

    pub fn repositories(&self) -> SectionState<ProfilePinned> {
        self.ui.with(|ui| ui.repositories.clone())
    }

    pub fn contributions(&self) -> SectionState<ContributionCalendar> {
        self.ui.with(|ui| ui.contributions.clone())
    }

    pub fn readme_state(&self) -> SectionState<()> {
        self.ui.with(|ui| ui.readme.clone())
    }

    pub fn basic_provenance(&self) -> Option<ProfileProvenance> {
        self.ui.with(|ui| ui.basic_provenance.clone())
    }

    pub fn repositories_provenance(&self) -> Option<ProfileProvenance> {
        self.ui.with(|ui| ui.repositories_provenance.clone())
    }

    pub fn contributions_provenance(&self) -> Option<ProfileProvenance> {
        self.ui.with(|ui| ui.contributions_provenance.clone())
    }

    pub fn readme_provenance(&self) -> Option<ProfileProvenance> {
        self.ui.with(|ui| ui.readme_provenance.clone())
    }

    pub fn avatar(&self) -> Option<ImageSrc> {
        self.ui.with(|ui| ui.avatar.clone())
    }

    pub fn set_now(&mut self, now: SystemTime) {
        self.ui.update(|ui| ui.now = now);
    }

    pub fn selected_day_detail(&self) -> Option<String> {
        let ui = self.ui.get_untracked();
        let selected = ui.selected_day?;
        heatmap_model(&ui.range, &ui.contributions).and_then(|model| {
            model
                .weeks
                .iter()
                .flat_map(|week| week.days.iter())
                .nth(selected)
                .map(|day| day.detail())
        })
    }

    /// Publish a confirmed non-error absence (no eligible public root README).
    pub fn set_readme_unavailable_with_provenance(
        &mut self,
        request: &ProfileRequest,
        message: impl Into<String>,
        provenance: Option<ProfileProvenance>,
    ) -> bool {
        if !self.accepts(request) {
            return false;
        }
        self.drop_reader();
        let message = message.into();
        self.ui.update(|ui| {
            ui.readme = SectionState::Unavailable(message);
            ui.readme_provenance = provenance;
            ui.readme_identity = None;
        });
        true
    }

    /// A real failure keeps its typed state and one compact warning mark. No
    /// absence is turned into an error and no error into a quiet absence.
    pub fn set_readme_error(
        &mut self,
        request: &ProfileRequest,
        error: super::PresentationError,
        identity: Option<String>,
        provenance: Option<ProfileProvenance>,
    ) -> bool {
        if !self.accepts(request) {
            return false;
        }
        self.drop_reader();
        self.ui.update(|ui| {
            ui.readme = SectionState::Error(error);
            ui.readme_provenance = provenance;
            ui.readme_identity = identity;
        });
        true
    }

    /// Installs a prepared README. The document was parsed off the UI thread; the
    /// image bridge carries the captured authorization and the shared provider.
    #[allow(clippy::too_many_arguments)]
    pub fn set_readme_document(
        &mut self,
        request: &ProfileRequest,
        document: Arc<ReaderDocument>,
        provider: Option<Arc<dyn crate::content::images::ImageProvider>>,
        guard: Arc<dyn Fn() -> bool + Send + Sync>,
        provenance: Option<ProfileProvenance>,
    ) -> bool {
        if !self.accepts(request) {
            return false;
        }
        self.drop_reader();
        let identity = format!("{} / {}", document.context.owner, document.context.path);
        let action_slot = self.on_action.clone();
        let request = self.request();
        let ok = Rc::new(std::cell::Cell::new(true));
        let action_ok = ok.clone();
        let bridge = ImageBridge::new(
            provider,
            Arc::new(move || (guard)()),
            crate::content::images::ImageLimits::default(),
        );
        let handle = ReaderHandle::new(document, bridge, ReaderLayout::Flow, move |action| {
            if !action_ok.get() || action_slot.borrow().is_none() {
                return;
            }
            let action = super::ProfileAction {
                request: request.clone(),
                kind: ProfileActionKind::Readme(action),
            };
            if let Some(sink) = action_slot.borrow().as_ref() {
                sink(action);
            }
        });
        *self.reader.borrow_mut() = Some(handle);
        self.ui.update(|ui| {
            ui.readme = SectionState::Ready(());
            ui.readme_provenance = provenance;
            ui.readme_identity = Some(identity);
        });
        self.reader_epoch.update(|epoch| *epoch += 1);
        true
    }

    pub fn set_avatar_image(&mut self, request: &ProfileRequest, image: Option<ImageSrc>) -> bool {
        if !self.accepts(request) {
            return false;
        }
        if image.is_none() {
            return false;
        }
        self.ui.update(|ui| ui.avatar = image);
        true
    }

    /// Polls the mounted reader. Pure memory: the bridge validates the captured
    /// authorization, accepts completed pictures within budget and queues new
    /// downloads on the existing provider. No storage or network call happens here.
    pub fn poll_reader(&self) -> bool {
        let changed = self
            .reader
            .borrow()
            .as_ref()
            .is_some_and(|reader| reader.poll());
        if changed {
            self.reader_epoch.update(|epoch| *epoch += 1);
        }
        changed
    }

    pub fn has_pending_images(&self) -> bool {
        self.reader
            .borrow()
            .as_ref()
            .is_some_and(|reader| reader.has_pending_images())
    }

    pub fn drop_reader(&mut self) {
        if let Some(reader) = self.reader.borrow_mut().take() {
            reader.retire();
            self.reader_epoch.update(|epoch| *epoch += 1);
        }
    }

    /// Stops reader image work while the page remains mounted but inactive. Valid
    /// content and the attached provider stay intact.
    pub fn suspend(&self) {
        if let Some(reader) = self.reader.borrow().as_ref() {
            reader.suspend();
        }
    }

    pub fn on_action(&mut self, handler: OnProfileAction) {
        *self.on_action.borrow_mut() = Some(handler);
    }

    pub(super) fn emit(&self, kind: ProfileActionKind) {
        let action = super::ProfileAction {
            request: self.request(),
            kind,
        };
        if let Some(sink) = self.on_action.borrow().as_ref() {
            sink(action);
        }
    }

    pub(crate) fn update_basic_with_provenance(
        &mut self,
        request: &ProfileRequest,
        state: SectionState<ProfileBasic>,
        provenance: Option<ProfileProvenance>,
    ) -> bool {
        if !self.accepts(request) {
            return false;
        }
        self.ui.update(|ui| {
            ui.basic = state;
            ui.basic_provenance = provenance;
        });
        true
    }

    pub(crate) fn update_repositories_with_provenance(
        &mut self,
        request: &ProfileRequest,
        state: SectionState<ProfilePinned>,
        provenance: Option<ProfileProvenance>,
    ) -> bool {
        if !self.accepts(request) {
            return false;
        }
        self.ui.update(|ui| {
            ui.repositories = state;
            ui.repositories_provenance = provenance;
        });
        true
    }

    pub(crate) fn update_contributions_with_provenance(
        &mut self,
        request: &ProfileRequest,
        range: &ContributionRange,
        state: SectionState<ContributionCalendar>,
        provenance: Option<ProfileProvenance>,
    ) -> bool {
        if !self.accepts(request) || range != &self.range() {
            return false;
        }
        self.ui.update(|ui| {
            ui.selected_day = match &state {
                SectionState::Ready(calendar) if !calendar.weeks.is_empty() => Some(0),
                _ => None,
            };
            ui.contributions = state;
            ui.contributions_provenance = provenance;
        });
        true
    }

    /// Adopt a different contribution interval without clearing the other
    /// sections: an offline snapshot may have been stored under an earlier
    /// rolling year, and the visible interval must describe the data on screen.
    pub fn set_range(&mut self, range: ContributionRange) -> bool {
        if self.range() == range {
            return false;
        }
        self.ui.update(|ui| {
            ui.range = range;
            ui.selected_day = None;
        });
        true
    }

    /// Element for the whole page. Pure presentation over the published signal.
    pub fn element(&self) -> AnyElement {
        super::render::page(self)
    }
}

pub(super) fn heatmap_model(
    range: &ContributionRange,
    contributions: &SectionState<ContributionCalendar>,
) -> Option<HeatmapPresentation> {
    match contributions {
        SectionState::Ready(calendar) => {
            Some(super::heatmap::heatmap_presentation(range, calendar))
        }
        _ => None,
    }
}

pub(super) fn repository_presentation_of(
    repositories: &SectionState<ProfilePinned>,
) -> Option<crate::ui::profile::RepositoryPresentation> {
    match repositories {
        SectionState::Ready(pinned) => Some(repository_presentation(pinned)),
        _ => None,
    }
}

/// The mounted reader element, if one is installed. Re-evaluated when the reader
/// epoch changes so a replaced document repaints.
pub(super) fn reader_element(view: &ProfileView) -> Option<AnyElement> {
    let _ = view.reader_epoch.get();
    view.reader
        .borrow()
        .as_ref()
        .map(|handle| reader(handle.clone()))
}
