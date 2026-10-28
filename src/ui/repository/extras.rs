//! Repository-scoped overview composition. Branch changes never restart these reads.
use super::{PageAction, RepositoryPage, Work};
use crate::data::{PageReadSession, RepositoryReadContext};
use crate::domain::repository::RepositoryIdentity;
use crate::github::RepositoryExtrasLimits;
use crate::ui::repository_overview::{
    OverviewAction, OverviewBinding, OverviewSection, OverviewUpdate, ReleasesData,
    RepositoryOverview, SectionState,
};
use gpui::{AppContext, Context, Entity, Window};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug)]
enum Endpoint {
    Releases,
    Latest,
    Contributors,
    Languages,
}
impl Endpoint {
    fn index(self) -> usize {
        match self {
            Self::Releases => 0,
            Self::Latest => 1,
            Self::Contributors => 2,
            Self::Languages => 3,
        }
    }
}
struct Reply {
    binding: OverviewBinding,
    context: RepositoryReadContext,
    endpoint: Endpoint,
    request: u64,
    update: OverviewUpdate,
}
struct CountsReply {
    binding: OverviewBinding,
    context: RepositoryReadContext,
    request: u64,
    result: Result<
        crate::domain::repository_counts::RepositoryCounts,
        crate::domain::repository_counts::CountsError,
    >,
}
#[derive(Default)]
pub(super) struct Extras {
    pub view: Option<Entity<RepositoryOverview>>,
    session: Option<PageReadSession>,
    binding: Option<OverviewBinding>,
    requests: [u64; 4],
    work: [Option<Work>; 4],
    inbox: Arc<Mutex<VecDeque<Reply>>>,
    pub counts: Option<crate::domain::repository_counts::RepositoryCounts>,
    counts_work: Option<Work>,
    counts_inbox: Arc<Mutex<VecDeque<CountsReply>>>,
    counts_request: u64,
}
impl Extras {
    pub fn stop(&mut self) {
        for work in &mut self.work {
            if let Some(work) = work.take() {
                work.source.cancel();
            }
        }
        if let Some(work) = self.counts_work.take() {
            work.source.cancel();
        }
        self.counts = None;
        self.counts_request += 1;
        self.view = None;
        self.session = None;
        self.binding = None;
        self.inbox.lock().expect("overview inbox").clear();
    }
    fn accepts(&self, page: &RepositoryPage, binding: &OverviewBinding) -> bool {
        page.active
            && self.binding.as_ref() == Some(binding)
            && binding.generation == page.generation
            && page
                .details
                .as_ref()
                .is_some_and(|details| details.identity == binding.repository)
            && self.session.as_ref().is_some_and(|session| {
                Some(session.context()) == page.context.as_ref() && session.verify()
            })
    }
}
impl RepositoryPage {
    pub(super) fn start_overview(
        &mut self,
        repository: RepositoryIdentity,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.extras.stop();
        let binding = OverviewBinding::new(
            self.context
                .as_ref()
                .expect("repository context")
                .auth()
                .account
                .clone(),
            repository,
            self.generation,
        );
        let view = cx.new(|cx| RepositoryOverview::new(binding.clone(), window, cx));
        let owner = cx.weak_entity();
        view.update(cx, |view, _| {
            view.on_action(std::rc::Rc::new(move |action, window, cx| {
                let owner = owner.clone();
                window.defer(cx, move |window, cx| {
                    let _ = owner.update(cx, |page, cx| page.overview_action(action, window, cx));
                });
            }))
        });
        self.extras.view = Some(view);
        self.extras.binding = Some(binding.clone());
        let session = self.services.accounts.page_session();
        match session {
            Ok(session) if Some(session.context()) == self.context.as_ref() && session.verify() => {
                self.extras.session = Some(session)
            }
            _ => {
                if let Some(view) = &self.extras.view {
                    view.update(cx, |view, cx| {
                        let message =
                            "Authorization changed. Activate this page again.".to_string();
                        view.apply(
                            &binding,
                            OverviewUpdate::Releases(SectionState::Error {
                                message: message.clone(),
                            }),
                            cx,
                        );
                        view.apply(
                            &binding,
                            OverviewUpdate::Contributors(SectionState::Error {
                                message: message.clone(),
                            }),
                            cx,
                        );
                        view.apply(
                            &binding,
                            OverviewUpdate::Languages(SectionState::Error { message }),
                            cx,
                        );
                    });
                }
                self.check_authorization(cx);
                return;
            }
        }
        self.read_counts();
        for endpoint in [
            Endpoint::Releases,
            Endpoint::Latest,
            Endpoint::Contributors,
            Endpoint::Languages,
        ] {
            self.read_extra(endpoint, cx);
        }
    }
    fn read_counts(&mut self) {
        let Some(session) = self.extras.session.clone() else {
            return;
        };
        let binding = self.extras.binding.clone().unwrap();
        let context = session.context().clone();
        let inbox = self.extras.counts_inbox.clone();
        self.extras.counts_request += 1;
        let request = self.extras.counts_request;
        let work = Work::new();
        let cancel = work.token();
        self.extras.counts_work = Some(work);
        let _ = self.services.executor.handle().spawn(async move {
            let result = session
                .repository_counts(&binding.repository, cancel)
                .await
                .map(|value| value.value);
            inbox.lock().expect("count inbox").push_back(CountsReply {
                binding,
                context,
                request,
                result,
            });
        });
    }
    fn read_extra(&mut self, endpoint: Endpoint, cx: &mut Context<Self>) {
        let Some(binding) = self.extras.binding.clone() else {
            return;
        };
        if !self.extras.accepts(self, &binding) {
            return;
        }
        let index = endpoint.index();
        if let Some(work) = self.extras.work[index].take() {
            work.source.cancel();
        }
        self.extras.requests[index] += 1;
        let request = self.extras.requests[index];
        let work = Work::new();
        let cancel = work.token();
        self.extras.work[index] = Some(work);
        let loading = match endpoint {
            Endpoint::Releases => OverviewUpdate::Releases(SectionState::Loading),
            Endpoint::Latest => OverviewUpdate::LatestRelease(SectionState::Loading),
            Endpoint::Contributors => OverviewUpdate::Contributors(SectionState::Loading),
            Endpoint::Languages => OverviewUpdate::Languages(SectionState::Loading),
        };
        self.extras.view.as_ref().unwrap().update(cx, |view, cx| {
            view.apply(&binding, loading, cx);
        });
        let session = self.extras.session.clone().unwrap();
        let context = session.context().clone();
        let inbox = self.extras.inbox.clone();
        let alive = self.alive.clone();
        let repository = binding.repository.clone();
        let submitted = self.services.executor.handle().spawn(async move {
            let limits = RepositoryExtrasLimits::default();
            let update = match endpoint {
                Endpoint::Releases => OverviewUpdate::Releases(
                    match session.releases(&repository, &limits, cancel).await {
                        Ok(value) => SectionState::Ready(ReleasesData::new(value.value, None)),
                        Err(error) => SectionState::Error {
                            message: error.to_string(),
                        },
                    },
                ),
                Endpoint::Latest => OverviewUpdate::LatestRelease(
                    match session.latest_release(&repository, &limits, cancel).await {
                        Ok(value) => SectionState::Ready(value.value),
                        Err(error) => SectionState::Error {
                            message: error.to_string(),
                        },
                    },
                ),
                Endpoint::Contributors => OverviewUpdate::Contributors(
                    match session.contributors(&repository, &limits, cancel).await {
                        Ok(value) => SectionState::Ready(value.value),
                        Err(error) => SectionState::Error {
                            message: error.to_string(),
                        },
                    },
                ),
                Endpoint::Languages => OverviewUpdate::Languages(
                    match session.languages(&repository, &limits, cancel).await {
                        Ok(value) => SectionState::Ready(value.value),
                        Err(error) => SectionState::Error {
                            message: error.to_string(),
                        },
                    },
                ),
            };
            if *alive.borrow() {
                inbox.lock().expect("overview inbox").push_back(Reply {
                    binding,
                    context,
                    endpoint,
                    request,
                    update,
                });
            }
        });
        if submitted.is_err() {
            let message = "Background services are closing.".to_string();
            let update = match endpoint {
                Endpoint::Releases => OverviewUpdate::Releases(SectionState::Error { message }),
                Endpoint::Latest => OverviewUpdate::LatestRelease(SectionState::Error { message }),
                Endpoint::Contributors => {
                    OverviewUpdate::Contributors(SectionState::Error { message })
                }
                Endpoint::Languages => OverviewUpdate::Languages(SectionState::Error { message }),
            };
            let binding = self.extras.binding.as_ref().unwrap();
            self.extras.view.as_ref().unwrap().update(cx, |view, cx| {
                view.apply(binding, update, cx);
            });
        }
    }
    pub(super) fn drain_overview(&mut self, cx: &mut Context<Self>) {
        let counts: Vec<_> = self
            .extras
            .counts_inbox
            .lock()
            .expect("count inbox")
            .drain(..)
            .collect();
        for reply in counts {
            if reply.request == self.extras.counts_request
                && self.extras.accepts(self, &reply.binding)
                && self.context.as_ref() == Some(&reply.context)
                && self
                    .services
                    .accounts
                    .repo_service()
                    .is_current(&reply.context)
            {
                self.extras.counts = reply
                    .result
                    .ok()
                    .filter(|value| value.repository == reply.binding.repository);
                cx.notify();
            }
        }
        let replies: Vec<_> = self
            .extras
            .inbox
            .lock()
            .expect("overview inbox")
            .drain(..)
            .collect();
        for reply in replies {
            if self.extras.requests[reply.endpoint.index()] != reply.request
                || !self.extras.accepts(self, &reply.binding)
                || self.context.as_ref() != Some(&reply.context)
                || !self
                    .services
                    .accounts
                    .repo_service()
                    .is_current(&reply.context)
            {
                continue;
            }
            self.extras.view.as_ref().unwrap().update(cx, |view, cx| {
                view.apply(&reply.binding, reply.update, cx);
            });
            // The language strip is composed by the page from this same state.
            cx.notify();
        }
    }
    pub(super) fn overview_action(
        &mut self,
        action: OverviewAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let binding = match &action {
            OverviewAction::Refresh { binding, .. }
            | OverviewAction::OpenRelease { binding, .. }
            | OverviewAction::OpenContributor { binding, .. } => binding,
        };
        if !self.extras.accepts(self, binding) {
            return;
        }
        match action {
            OverviewAction::Refresh { section, .. } => match section {
                OverviewSection::Releases => {
                    self.read_extra(Endpoint::Releases, cx);
                    self.read_extra(Endpoint::Latest, cx);
                }
                OverviewSection::Contributors => self.read_extra(Endpoint::Contributors, cx),
                OverviewSection::Languages => self.read_extra(Endpoint::Languages, cx),
            },
            action => {
                let Some(view) = self.extras.view.as_ref() else {
                    return;
                };
                let view = view.read(cx);
                let valid = match &action {
                    OverviewAction::OpenRelease {
                        release_id,
                        tag_name,
                        html_url,
                        ..
                    } => view
                        .latest_release()
                        .and_then(SectionState::value)
                        .and_then(|latest| latest.release.as_ref())
                        .is_some_and(|release| {
                            release.id == *release_id
                                && release.tag_name == *tag_name
                                && release.html_url == *html_url
                        }),
                    OverviewAction::OpenContributor {
                        login, html_url, ..
                    } => view.contributors().value().is_some_and(|value| {
                        value.contributors.iter().any(|person| {
                            person.login == *login
                                && person.html_url == *html_url
                                && person.html_url.is_some()
                        })
                    }),
                    _ => false,
                };
                if valid {
                    self.emit(PageAction::Overview(action), window, cx);
                }
            }
        }
    }
    pub(super) fn repository_binding(&self) -> Option<OverviewBinding> {
        self.extras
            .binding
            .clone()
            .filter(|binding| self.extras.accepts(self, binding))
    }
    pub fn overview(&self) -> Option<&Entity<RepositoryOverview>> {
        self.extras.view.as_ref()
    }
}
impl Drop for Extras {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::repository::tests::{frames, normal};
    use gpui::TestAppContext;
    #[gpui::test]
    fn stale_count_error_does_not_replace_newer_value_or_new_authorization(
        cx: &mut TestAppContext,
    ) {
        let (fixture, page, cx) = normal(cx);
        page.update(cx, |page, cx| {
            let binding = page.extras.binding.clone().unwrap();
            let context = page.context.clone().unwrap();
            let value = crate::domain::repository_counts::RepositoryCounts {
                repository: binding.repository.clone(),
                issues: Ok(3),
                pull_requests: Ok(1),
            };
            let current = page.extras.counts_request;
            page.extras.counts = Some(value.clone());
            page.extras
                .counts_inbox
                .lock()
                .unwrap()
                .push_back(CountsReply {
                    binding,
                    context,
                    request: current - 1,
                    result: Err(crate::domain::repository_counts::CountsError::RepositoryIdentity),
                });
            page.drain_overview(cx);
            assert_eq!(page.extras.counts, Some(value));
        });
        fixture.login_as(8, "new");
        page.update_in(cx, |page, w, cx| page.drain(w, cx));
        assert!(page.read_with(cx, |page, _| page.extras.counts.is_none()));
    }
    #[gpui::test]
    fn stale_endpoint_result_and_error_cannot_overwrite_retry_or_new_binding(
        cx: &mut TestAppContext,
    ) {
        let (_fixture, page, cx) = normal(cx);
        page.update(cx, |page, cx| {
            let binding = page.extras.binding.clone().unwrap();
            let context = page.context.clone().unwrap();
            page.read_extra(Endpoint::Languages, cx);
            let request = page.extras.requests[3];
            page.extras.inbox.lock().unwrap().push_back(Reply {
                binding: binding.clone(),
                context: context.clone(),
                endpoint: Endpoint::Languages,
                request: request - 1,
                update: OverviewUpdate::Languages(SectionState::Error {
                    message: "old error".into(),
                }),
            });
            page.drain_overview(cx);
            assert!(
                page.extras
                    .view
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .languages()
                    .is_loading()
            );
            page.deactivate(cx);
            page.extras.inbox.lock().unwrap().push_back(Reply {
                binding,
                context,
                endpoint: Endpoint::Languages,
                request,
                update: OverviewUpdate::Languages(SectionState::Error {
                    message: "after deactivate".into(),
                }),
            });
            page.drain_overview(cx);
            assert!(page.extras.view.is_none());
        });
        frames(cx);
    }
}
