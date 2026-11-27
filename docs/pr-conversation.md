# PR Conversation

`Route::PullRequest { namespace, name, number }` mounts `PullRequestPage` in the Shell. The base repository and PR number identify the resource. The Feed's existing **Read more** opens the page and reuses it within the same account. The workspace retains the tab and scroll position on ordinary tab switches. Account changes capture new read sessions.

`RepositorySession::pr_conversation` uses the existing credential, cancellation and network boundaries. GitHub GraphQL `state` distinguishes open, closed without merging and merged. `isDraft` identifies drafts among open PRs. The summary shows the author, base repository and branch, and source repository and branch. A visible head repository with a null current `headRef` displays **Branch deleted** with the historical `headRefName`. An unavailable head repository gets its own label. Partial GraphQL replies with errors fail the read. [GitHub PullRequest object](https://docs.github.com/en/graphql/reference/pulls#pullrequest)

The page uses the shared Markdown Flow reader and account-scoped attachments provider for the description and ordinary comments. Repository-relative Markdown links open the base repository's default-branch file on GitHub and keep the fragment. Blank descriptions display **No description provided.** The comments use the #106 `CommentsRequest` and `Controller` with the PR's base repository and number. **Load more comments** follows the shared validated cursor. Empty success displays **No comments yet.** A failed page offers a retry and retains previously loaded comments when access remains valid. [GitHub ordinary comments endpoint](https://docs.github.com/en/rest/issues/comments#list-issue-comments)

The Shell refresh rechecks PR details and then the first page of comments while keeping visible content. The PR mounts only after capturing matching account scopes for both services.

## Offline Conversation

The formal page reads authorized snapshots from the app's `CacheStore` before requesting GitHub. The normalized route resolves to a numeric repository ID in the saved account's partition. The body and ordinary comment pages use that repository's authorization gate. When a replacement PAT has cleared the route index and quarantined the repository, a successful PR read identifies the repository and a fresh metadata request proves access to that same numeric ID before caching resumes. Keys include the PR number and API representation. Comments also include the requested page and page size. The snapshot contains the description and the summary fields used to read it in context. Checks, approvals, changes and write operations are outside Conversation storage.

A restart restores the contiguous prefix of up to ten previously read comment pages from the same first-page generation. A gap, further unread pages or the replay limit leaves the list partial. A successful refresh replaces the displayed comment collection from page one, and **Load more comments** follows the new validated cursor. The cache never joins continuations from different first-page generations.

The body and comments remain visible while refresh runs. If a transient failure leaves a saved snapshot visible, that section shows **Cached** with a retry. A successful retry replaces the stale content. An access denial or invisible resource from either request clears the page. Credential rejection uses the shared account lifecycle, and repository revocation invalidates the cached display authorization. A `no-store` response, including a failed response, invalidates the corresponding durable snapshot. The page can still read online when cache storage or binding is unavailable.

## Integration and verification

The Conversation change was reviewed as a stack on `feat/issue-detail` at `ab24dd949424d392400abc5b5cb23828b662039d`. After [#106](https://github.com/nocpiun/githux/pull/106) was squash-merged as `951da323a114d3d2dd6e8cb2955779c12f2f1a13`, [#109](https://github.com/nocpiun/githux/pull/109) moved to `main` with only the Conversation changes. The consumer branch is `feat/pr-conversation`. The Home work-item and notification subject navigation producers can target the same `Route::PullRequest` when their branches are integrated. Their observed heads were `b346164ac7cf0328523367b3f66a8ba5ddf18fe1` and `0126de1e470ac6aabe315347db6fc752d78fb422`. [Home navigation #107](https://github.com/nocpiun/githux/pull/107) · [Notification navigation #105](https://github.com/nocpiun/githux/pull/105)

```sh
node scripts/verify.mjs -- cargo test --locked --bin githux app::pages:: -- --test-threads=1
node scripts/verify.mjs -- cargo run --locked
```

The Shell fixture clicks the existing Feed **Read more** for `alice/demo #5` and checks its actual Markdown description and ordinary comments. The Windows acceptance path uses the same action in a signed-in account with an expanded PR preview in Feed. Final visual review is part of the combined Issue/PR reading batch.

Files changed can compose a second content surface under the same PR header in `PullRequestPage`. The file reader will receive the PR target and its account-scoped session. The current page shows Conversation directly.
