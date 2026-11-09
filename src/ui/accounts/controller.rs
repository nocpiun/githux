//! Account surface state machine. No NSH signals, services, credentials or I/O: the NSH view
//! enqueues effects, `poll` submits them to the one [`RuntimeHandle`], and the authoritative
//! snapshot is ingested back here.
//!
//! This is the native replacement for the old GPUI controller bridge. It keeps the confirmed
//! behaviour: a PAT moves out of the input before the command is submitted, no operation is
//! submitted twice, cancellations settle in the service and never claim a rollback, and the
//! directory in the snapshot is authoritative (zero accounts clears every saved row).

use crate::app::runtime::{
    Command, Completion, Failure, Operation, RepositoryList, Snapshot, Startup, SubmitError,
    Success,
};
use crate::auth::token::Pat;
use crate::domain::account::AuthorizationState;
use crate::domain::{AccountKey, AccountSummary};
use crate::storage::cache::CacheError;
use std::collections::VecDeque;

/// The official Classic PAT creation page. Opening it creates no token and reads no credential.
pub const PAT_HELP_URL: &str =
    "https://github.com/settings/personal-access-tokens/new?type=classic";

/// Which form the surface is showing. Replacement targets one saved account, never implicitly the
/// active one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Add,
    Replace(AccountKey),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Status {
    pub tone: Tone,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Recovery {
    None,
    Pending,
    Unreadable(String),
}

/// Work the UI thread hands to the runtime on the next poll. `Command` owns the PAT; it is never
/// formatted, logged or put into a snapshot.
pub(crate) enum Effect {
    Submit(Command),
    Cancel,
    OpenExternal(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Presentation {
    pub startup: Startup,
    pub accounts: Vec<AccountSummary>,
    pub active: Option<AccountKey>,
    pub repositories: RepositoryList,
    pub busy: bool,
    pub settling: bool,
    pub mode: Mode,
    pub form_open: bool,
    pub status: Option<Status>,
    pub confirm_remove: Option<AccountKey>,
    pub help_open: bool,
    pub input_empty: bool,
    pub recovery: Recovery,
    pub cache_error: Option<CacheError>,
}

/// The snapshot before the background services answer. It is honest: no account, no list, opening.
pub(crate) fn empty_snapshot() -> Snapshot {
    Snapshot {
        startup: Startup::Opening,
        accounts: Vec::new(),
        active: None,
        repositories: RepositoryList::Idle,
        busy: None,
        settling: false,
        completion: None,
        recovery_pending: Ok(false),
        cache_open_error: None,
        binding_revision: 0,
    }
}

pub(crate) struct AccountsController {
    snapshot: Snapshot,
    input: String,
    mode: Mode,
    status: Option<Status>,
    confirm_remove: Option<AccountKey>,
    help_open: bool,
    form_open: bool,
    awaiting: Option<Operation>,
    effects: VecDeque<Effect>,
    /// A command is queued or in flight; it keeps a second submission from being built before the
    /// runtime reports its own busy state.
    queued_mutation: bool,
    last_completion: Option<Completion>,
    last: Presentation,
}

impl AccountsController {
    pub fn new() -> Self {
        let snapshot = empty_snapshot();
        let mut this = Self {
            snapshot,
            input: String::new(),
            mode: Mode::Add,
            status: None,
            confirm_remove: None,
            help_open: false,
            form_open: false,
            awaiting: None,
            effects: VecDeque::new(),
            queued_mutation: false,
            last_completion: None,
            last: Presentation {
                startup: Startup::Opening,
                accounts: Vec::new(),
                active: None,
                repositories: RepositoryList::Idle,
                busy: false,
                settling: false,
                mode: Mode::Add,
                form_open: true,
                status: None,
                confirm_remove: None,
                help_open: false,
                input_empty: true,
                recovery: Recovery::None,
                cache_error: None,
            },
        };
        this.last = this.presentation();
        this
    }

    pub fn presentation(&self) -> Presentation {
        let busy = self.busy();
        Presentation {
            startup: self.snapshot.startup.clone(),
            accounts: self.snapshot.accounts.clone(),
            active: self.snapshot.active.clone(),
            repositories: self.snapshot.repositories.clone(),
            busy,
            settling: self.snapshot.settling,
            mode: self.mode.clone(),
            form_open: self.form_open || self.snapshot.accounts.is_empty(),
            status: self.status.clone(),
            confirm_remove: self.confirm_remove.clone(),
            help_open: self.help_open,
            input_empty: self.input.trim().is_empty(),
            recovery: match &self.snapshot.recovery_pending {
                Ok(false) => Recovery::None,
                Ok(true) => Recovery::Pending,
                Err(error) => Recovery::Unreadable(error.to_string()),
            },
            cache_error: self.snapshot.cache_open_error,
        }
    }

    pub fn busy(&self) -> bool {
        self.queued_mutation || self.snapshot.busy.is_some() || self.snapshot.settling
    }

    /// The live masked field text. It is never rendered anywhere else and is cleared as soon as a
    /// command owns it.
    pub fn set_input(&mut self, text: &str) {
        self.input.clear();
        self.input.push_str(text);
    }

    /// The transient masked text, for the controlled input. Nothing else renders it.
    pub fn input_text(&self) -> &str {
        &self.input
    }

    /// The runtime refused a queued command before running it. The command is gone; the form stays
    /// usable so the user can retry after the reported condition clears.
    pub fn note_submit_error(&mut self, error: SubmitError) {
        self.queued_mutation = false;
        let text = match error {
            SubmitError::NotReady => {
                "Account services are not ready yet; the operation was not submitted."
            }
            SubmitError::Busy => "Another account operation is already in progress.",
            SubmitError::Closing => {
                "Background services are closing; the operation was not submitted."
            }
            SubmitError::EmptyToken => "Paste a Classic PAT before submitting.",
            SubmitError::Worker => {
                "The operation was not submitted because background services are closing."
            }
        };
        self.status = Some(Status {
            tone: Tone::Error,
            text: text.into(),
        });
    }

    /// Drops queued work and the transient secret when the surface is retired.
    pub fn retire(&mut self) {
        *self = Self::new();
        self.snapshot.startup = Startup::Closed;
    }

    /// Marks a command accepted by RuntimeHandle. Completion may arrive together with an automatic
    /// repository load, so it must be consumed even while that read is busy.
    pub fn submitted(&mut self, operation: Operation) {
        if operation != Operation::Repositories {
            self.awaiting = Some(operation);
        }
    }

    /// Hands the queued work to the poll loop. The submission guard stays up until the poll ends,
    /// so a second click before the runtime reports its own busy state cannot queue another command.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        self.effects.drain(..).collect()
    }

    /// Ends one poll after the snapshot was ingested. The runtime's busy/settling state now guards
    /// further submissions.
    pub fn end_poll(&mut self) {
        self.queued_mutation = false;
    }

    /// Builds one command from the current field. False means nothing was queued: the field was
    /// empty, or another operation is already in flight (double submission).
    pub fn submit(&mut self, text: &str) -> bool {
        if self.busy() {
            return false;
        }
        let trimmed = text.trim();
        if trimmed.is_empty() {
            self.status = Some(Status {
                tone: Tone::Error,
                text: "Paste a Classic PAT before submitting.".into(),
            });
            return false;
        }
        let token = Pat::from_str(trimmed);
        let command = match &self.mode {
            Mode::Add => Command::Add(token),
            Mode::Replace(account) => Command::Replace {
                account: account.clone(),
                token,
            },
        };
        let text = match &self.mode {
            Mode::Add => "Verifying the PAT before saving…",
            Mode::Replace(_) => "Verifying the replacement PAT…",
        };
        self.input.clear();
        self.effects.push_back(Effect::Submit(command));
        self.queued_mutation = true;
        self.status = Some(Status {
            tone: Tone::Info,
            text: text.into(),
        });
        true
    }

    /// Cancels an in-flight operation, or closes/reset the idle form. A cancelled mutation is not
    /// rolled back; its reply is suppressed and the service settles it.
    pub fn cancel(&mut self) {
        self.input.clear();
        if self.busy() {
            self.effects.push_back(Effect::Cancel);
            self.status = Some(Status {
                tone: Tone::Info,
                text: "Cancellation requested. An in-flight account operation still settles in the service; saved accounts are reconciled when it does, and nothing claims it was rolled back.".into(),
            });
            return;
        }
        self.reset_form();
        self.status = Some(Status {
            tone: Tone::Info,
            text: "Nothing was submitted; the saved PAT is unchanged.".into(),
        });
    }

    pub fn start_add(&mut self) {
        if !self.busy() {
            self.reset_form();
            self.form_open = true;
            self.status = None;
        }
    }

    pub fn toggle_help(&mut self) {
        self.help_open = !self.help_open;
    }

    pub fn reset_form(&mut self) {
        self.form_open = false;
        self.mode = Mode::Add;
        self.input.clear();
        self.help_open = false;
        self.confirm_remove = None;
    }

    pub fn select(&mut self, account: AccountKey) {
        if self.busy() || !self.exists(&account) {
            return;
        }
        self.reset_form();
        self.queued_mutation = true;
        self.effects
            .push_back(Effect::Submit(Command::Select(account)));
        self.status = Some(Status {
            tone: Tone::Info,
            text: "Saving the current account selection…".into(),
        });
    }

    pub fn start_replace(&mut self, account: AccountKey) {
        if self.busy() {
            return;
        }
        let Some((name, login)) = self
            .summary(&account)
            .map(|summary| (display_name(summary), summary.identity.login.clone()))
        else {
            self.status = Some(Status {
                tone: Tone::Error,
                text: "That saved account no longer exists.".into(),
            });
            return;
        };
        self.mode = Mode::Replace(account);
        self.form_open = true;
        self.input.clear();
        self.help_open = false;
        self.confirm_remove = None;
        self.status = Some(Status {
            tone: Tone::Info,
            text: format!(
                "Enter a new Classic PAT for {name} (@{login}). A token for a different account is rejected and neither account changes."
            ),
        });
    }

    pub fn revalidate(&mut self, account: AccountKey) {
        if self.busy() || !self.exists(&account) {
            return;
        }
        self.queued_mutation = true;
        self.effects
            .push_back(Effect::Submit(Command::Revalidate(account)));
        self.status = Some(Status {
            tone: Tone::Info,
            text: "Revalidating the saved credential…".into(),
        });
    }

    pub fn ask_remove(&mut self, account: AccountKey) {
        if self.busy() {
            return;
        }
        let Some((name, login)) = self
            .summary(&account)
            .map(|summary| (display_name(summary), summary.identity.login.clone()))
        else {
            return;
        };
        self.reset_form();
        self.confirm_remove = Some(account);
        self.status = Some(Status {
            tone: Tone::Warning,
            text: format!(
                "Remove {name} (@{login}) from Githux? Its saved credential will be removed and its cached data retired."
            ),
        });
    }

    pub fn confirm_remove(&mut self, account: AccountKey) {
        if self.busy() || self.confirm_remove.as_ref() != Some(&account) {
            return;
        }
        self.confirm_remove = None;
        self.queued_mutation = true;
        self.effects
            .push_back(Effect::Submit(Command::Remove(account)));
        self.status = Some(Status {
            tone: Tone::Info,
            text: "Removing the saved account…".into(),
        });
    }

    pub fn dismiss_remove(&mut self) {
        if self.confirm_remove.take().is_some() {
            self.status = Some(Status {
                tone: Tone::Info,
                text: "Removal cancelled. The saved account is unchanged.".into(),
            });
        }
    }

    pub fn recover(&mut self) {
        if self.busy() {
            return;
        }
        self.queued_mutation = true;
        self.effects.push_back(Effect::Submit(Command::Recover));
        self.status = Some(Status {
            tone: Tone::Info,
            text: "Retrying retired credential cleanup…".into(),
        });
    }

    pub fn retry_repositories(&mut self) {
        if self.busy() {
            return;
        }
        self.queued_mutation = true;
        self.effects
            .push_back(Effect::Submit(Command::RetryRepositories));
        self.status = Some(Status {
            tone: Tone::Info,
            text: "Reading all visible repository pages…".into(),
        });
    }

    pub fn open_pat_help(&mut self) {
        self.effects
            .push_back(Effect::OpenExternal(PAT_HELP_URL.into()));
    }

    /// Applies the authoritative snapshot. Returns true when anything the surface renders changed.
    /// Reading the whole snapshot every drain is what clears rows, the active account and the
    /// displayed repository list when the account is removed, replaced or revoked.
    pub fn ingest(&mut self, snapshot: &Snapshot) -> bool {
        let account_changed = self.snapshot.active != snapshot.active;
        if account_changed && self.awaiting.is_none() && !self.queued_mutation {
            self.reset_form();
            self.status = None;
        }
        self.apply_completion(snapshot);
        self.snapshot = snapshot.clone();
        if self.queued_mutation && snapshot.busy.is_some() {
            self.queued_mutation = false;
        }
        self.reconcile();
        let next = self.presentation();
        if next != self.last {
            self.last = next;
            true
        } else {
            false
        }
    }

    fn apply_completion(&mut self, snapshot: &Snapshot) {
        // A runtime retains the previous completion during the next mutation. Do not attribute it
        // to that mutation. A follow-up repository read, however, already carries the new result.
        if snapshot
            .busy
            .is_some_and(|op| op != Operation::Repositories)
        {
            return;
        }
        let completion = snapshot.completion.clone();
        let is_new =
            completion.is_some() && (completion != self.last_completion || self.awaiting.is_some());
        self.last_completion = completion.clone();
        let Some(completion) = completion.filter(|_| is_new) else {
            return;
        };
        let submitted = self.awaiting.take();
        match completion {
            Completion::Finished { operation, result } => match result {
                Ok(success) => {
                    self.apply_success(success);
                    if submitted == Some(operation)
                        && matches!(operation, Operation::Add | Operation::Replace)
                    {
                        self.reset_form();
                    }
                }
                Err(failure) => {
                    self.status = Some(Status {
                        tone: Tone::Error,
                        text: failure_text(&failure),
                    });
                    // A rejected replacement keeps its form open for correction; the secret itself
                    // was already cleared on submission.
                    if matches!(operation, Operation::Remove | Operation::Select) {
                        self.confirm_remove = None;
                    }
                }
            },
            Completion::CancelledSettled(_) => {
                self.status = Some(Status {
                    tone: Tone::Info,
                    text: "One cancelled operation settled. Saved accounts are shown as currently stored; nothing is claimed to have been rolled back.".into(),
                });
            }
        }
    }

    fn apply_success(&mut self, success: Success) {
        let (tone, text) = match success {
            Success::Added => (
                Tone::Success,
                "Account verified and saved; its PAT is in the platform credential store.",
            ),
            Success::Replaced => (Tone::Success, "Replacement PAT verified and saved."),
            Success::Selected => (Tone::Success, "Current account selection saved."),
            Success::Revalidated => (
                Tone::Success,
                "Saved credential revalidated against the current GitHub identity.",
            ),
            Success::Removed => (
                Tone::Success,
                "The saved account was removed and its cached data retired.",
            ),
            Success::Recovered => (Tone::Success, "Retired credential cleanup completed."),
            Success::CommittedCleanupPending => (
                Tone::Warning,
                "Credential cleanup remains pending. Check the saved accounts before retrying the change.",
            ),
        };
        if success == Success::Removed {
            self.confirm_remove = None;
        }
        self.status = Some(Status {
            tone,
            text: text.into(),
        });
    }

    /// Drops form/confirmation state that no longer refers to a saved account.
    fn reconcile(&mut self) {
        if let Some(key) = &self.confirm_remove
            && !self.exists(key)
        {
            self.confirm_remove = None;
        }
        if let Mode::Replace(key) = &self.mode
            && !self.exists(key)
        {
            self.mode = Mode::Add;
            self.input.clear();
            self.status = Some(Status {
                tone: Tone::Error,
                text: "The account being replaced is no longer saved.".into(),
            });
        }
    }

    fn exists(&self, key: &AccountKey) -> bool {
        self.snapshot
            .accounts
            .iter()
            .any(|account| &account.identity.key == key)
    }

    fn summary(&self, key: &AccountKey) -> Option<&AccountSummary> {
        self.snapshot
            .accounts
            .iter()
            .find(|account| &account.identity.key == key)
    }
}

pub(crate) fn display_name(summary: &AccountSummary) -> String {
    summary
        .identity
        .display_name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(&summary.identity.login)
        .to_string()
}

/// What the saved record currently proves. Only `Verified` is evidence of this run's access.
pub(crate) fn authorization_state(state: &AuthorizationState) -> (&'static str, bool) {
    match state {
        AuthorizationState::Verified => ("Verified this session", false),
        AuthorizationState::PendingVerification => ("Verification pending · Revalidate", true),
        AuthorizationState::MissingCredential => ("Credential missing · Replace PAT", true),
        AuthorizationState::CredentialUnavailable => {
            ("Credential unavailable · Revalidate or replace", true)
        }
        AuthorizationState::VerificationFailed(crate::domain::error::GitHubError::Unauthorized) => {
            ("Authorization expired or rejected · Replace PAT", true)
        }
        AuthorizationState::VerificationFailed(
            crate::domain::error::GitHubError::Network { .. }
            | crate::domain::error::GitHubError::Timeout { .. },
        ) => ("Could not connect · Revalidate when connected", true),
        AuthorizationState::VerificationFailed(_) => {
            ("Verification failed · Revalidate or replace", true)
        }
    }
}

pub(crate) fn startup_text(startup: &Startup) -> Option<(Tone, String)> {
    match startup {
        Startup::Opening => Some((Tone::Info, "Opening account services…".into())),
        Startup::Ready => None,
        Startup::Failed(error) => Some((Tone::Error, open_error_text(error))),
        Startup::Closed => Some((
            Tone::Warning,
            "Account services are closed. Restart Githux to manage accounts.".into(),
        )),
    }
}

fn open_error_text(error: &crate::app::runtime::OpenError) -> String {
    match error {
        crate::app::runtime::OpenError::Storage(error) => {
            format!("Account storage could not be opened: {error}. No saved account is shown.")
        }
        crate::app::runtime::OpenError::HttpClient => {
            "The HTTP client could not be created; account requests are unavailable.".into()
        }
        crate::app::runtime::OpenError::Worker => {
            "The background worker could not be started; account requests are unavailable.".into()
        }
    }
}

pub(crate) fn failure_text(failure: &Failure) -> String {
    match failure {
        Failure::Login(error) => match error {
            crate::data::LoginError::GitHub(error) => github_error_text(error),
            crate::data::LoginError::CredentialSave(error) => {
                format!("The PAT was not saved: {error}. The account directory was not changed.")
            }
            crate::data::LoginError::IdentityMismatch { .. } => {
                "This PAT belongs to a different account. Neither account was changed.".into()
            }
            crate::data::LoginError::NotFound => "That saved account no longer exists.".into(),
            crate::data::LoginError::Storage(error) => {
                format!("Account storage failed: {error}. No successful save is implied.")
            }
            crate::data::LoginError::CleanupPending(_) => {
                "The change was committed, but retired credential cleanup is pending.".into()
            }
        },
        Failure::Account(error) => match error {
            crate::data::AccountError::NotFound => "That saved account no longer exists.".into(),
            crate::data::AccountError::CleanupPending => {
                "Credential cleanup is still pending. Check the saved accounts before retrying."
                    .into()
            }
            crate::data::AccountError::Storage(error) => {
                format!("Account storage operation failed: {error}.")
            }
            crate::data::AccountError::Credential(error) => {
                format!("Platform credential operation failed: {error}.")
            }
        },
        Failure::GitHub(error) => github_error_text(error),
        Failure::Cache(error) => format!("The local cache rejected this operation: {error}."),
        Failure::NotCached(_) => {
            "No repository list is cached for this account; this is not an empty set.".into()
        }
        Failure::Worker => {
            "Background services are closing; the operation was not submitted.".into()
        }
    }
}

/// Never turns a failed read into a successful empty list.
pub(crate) fn github_error_text(error: &crate::domain::error::GitHubError) -> String {
    use crate::domain::error::GitHubError;
    match error {
        GitHubError::Unauthorized => "GitHub rejected this credential (401). Replace or revalidate the PAT; the account was not removed.".into(),
        GitHubError::RateLimited { retry } => format!(
            "GitHub rate limited this request ({:?}). Retry after {:?} seconds; this is not an empty result.",
            retry.kind, retry.seconds
        ),
        GitHubError::SsoRequired { authorization_url } => format!(
            "Organization SSO authorization is required. Authorize the PAT on GitHub{} and retry.",
            authorization_url
                .as_deref()
                .map(|url| format!(": {url}"))
                .unwrap_or_default()
        ),
        GitHubError::Forbidden => "GitHub denied access (403). The token scope or organization policy may be insufficient; no empty result is inferred.".into(),
        GitHubError::NotFound => "The resource is absent or not visible to this account (404).".into(),
        GitHubError::Network { .. } => "The network request failed. Check connectivity and retry; no result was established.".into(),
        GitHubError::Timeout { seconds } => format!("The request timed out after {seconds}s. Retry when ready."),
        GitHubError::Cancelled => "The request was cancelled; no result was applied.".into(),
        GitHubError::StaleContext => "The account or authorization changed while this request was running; its result was discarded.".into(),
        GitHubError::CredentialStore { .. } => "The platform credential is missing or unavailable. Revalidate or replace the PAT; this is not an empty result.".into(),
        other => format!("GitHub read failed: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_presentation_is_an_honest_logged_out_opening_state() {
        let controller = AccountsController::new();
        let presentation = controller.presentation();
        assert!(presentation.accounts.is_empty());
        assert!(presentation.active.is_none());
        assert_eq!(presentation.startup, Startup::Opening);
        assert!(presentation.input_empty);
        assert!(!presentation.busy);
    }
}
