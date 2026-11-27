# Offline PR Conversation

The formal PR page now uses the application cache for descriptions and already-read ordinary comments. The previous page held successful reads in memory, so mounting a fresh page after restarting required GraphQL and REST to succeed again. The snapshot lookup uses the saved account and normalized route, then applies the numeric repository gate to the actual reading data. No-store and visibility responses can invalidate the saved representation through its pre-request write ticket.

Comment pages have their own page keys. Replay stops at an unread page, a missing page, the end of that generation, or the ten-page replay budget. A fresh first page starts a new generation. A failed revalidation keeps authorized snapshots visible with a retry and a Cached indicator.

The Shell regression injects a temporary account database, in-memory test credentials and bounded mock HTTP through the production account/cache assembly. It covers first paint while the refresh is held, two previously read comment pages with a third still unread, cross-account isolation, offline failure and retry, and access denial followed by another offline reopen. The adapter test verifies invalidation after a failed no-store response and numeric repository revocation. PR results are delivered on the GPUI thread using the same background completion pattern as Issue detail.

The 25 formal page tests passed through `scripts/verify.mjs` in run `943a9219-fa49-4d1c-a548-258d0daa8e7c`. Formatting check passed in `3f189974-5466-40bf-b288-8055d279d667` and Clippy for the app and test targets passed in `640c9810-ab75-4e97-85c7-719fb1fdbcad`. Manual visual and signed-in acceptance are deferred to integrated main.
