# Android NativeActivity lifecycle patch

Source: crates.io `winit` 0.30.13, Apache-2.0 (see LICENSE).

Two changes are scoped to the Android backend:

- Exit the event loop on `MainEvent::Destroy`. NativeActivity's `onDestroy`
  waits for `android_main` to return, so ignoring the event blocks the UI thread.
- Release the one-event-loop guard when the Android event loop is dropped,
  allowing a later Activity to create its own loop in the same process.

The desktop and web paths are unchanged. The complete upstream source is kept
here for reproducible builds; no modifications to Cargo's global cache are needed.

Regression coverage: `android/tests/SmokeInstrumentation.java` finishes the
Activity, checks that the UI thread responds, opens a second Activity and verifies
the native editor remains usable. The unpatched version fails with a 15-second
main-thread timeout immediately after `finish()`.
