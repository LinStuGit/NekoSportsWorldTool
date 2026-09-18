package org.nekosportsworld.tool.tests;

import android.app.Activity;
import android.app.AlertDialog;
import android.app.Instrumentation;
import android.content.Intent;
import android.os.Bundle;
import android.text.InputType;
import android.view.WindowManager;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;
import android.widget.EditText;
import org.nekosportsworld.tool.MainActivity;

/** Offline Android integration checks. Never logs in or submits business data. */
public final class SmokeInstrumentation extends Instrumentation {
    private Throwable failure;
    private int passed;

    @Override public void onCreate(Bundle arguments) { super.onCreate(arguments); start(); }

    private void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
        passed++;
    }

    private void onUi(Runnable action) {
        java.util.concurrent.CountDownLatch done = new java.util.concurrent.CountDownLatch(1);
        new android.os.Handler(android.os.Looper.getMainLooper()).post(() -> {
            try { action.run(); } catch (Throwable error) { failure = error; }
            finally { done.countDown(); }
        });
        try {
            if (!done.await(15, java.util.concurrent.TimeUnit.SECONDS)) throw new AssertionError("Android main thread did not respond within 15 seconds");
        } catch (InterruptedException error) { throw new AssertionError(error); }
        if (failure != null) throw new AssertionError("UI check failed", failure);
    }

    private AlertDialog editor(MainActivity activity) {
        try {
            java.lang.reflect.Field field = MainActivity.class.getDeclaredField("editorDialog");
            field.setAccessible(true);
            return (AlertDialog) field.get(activity);
        } catch (ReflectiveOperationException error) { throw new AssertionError(error); }
    }

    private void awaitUi(java.util.function.BooleanSupplier condition, String description) throws InterruptedException {
        long deadline = android.os.SystemClock.uptimeMillis() + 15000;
        java.util.concurrent.atomic.AtomicBoolean ready = new java.util.concurrent.atomic.AtomicBoolean();
        while (android.os.SystemClock.uptimeMillis() < deadline) {
            onUi(() -> ready.set(condition.getAsBoolean()));
            if (ready.get()) return;
            Thread.sleep(100);
        }
        throw new AssertionError("Timed out waiting for " + description);
    }

    @Override public void onStart() {
        Bundle report = new Bundle();
        try {
            Intent launch = new Intent().setClassName("org.nekosportsworld.tool", "org.nekosportsworld.tool.MainActivity");
            launch.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
            MainActivity activity = (MainActivity) startActivitySync(launch);
            waitForIdleSync();
            awaitUi(() -> activity.hasWindowFocus(), "native activity window focus");
            onUi(() -> {
                check(activity.getFilesDir().isDirectory(), "private storage directory");
                check((activity.getWindow().getAttributes().flags & WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON) == 0, "idle must permit screen timeout");
                activity.setTaskActive(true);
                check((activity.getWindow().getAttributes().flags & WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON) != 0, "active task must keep screen on");
                activity.setTaskActive(false);
                check((activity.getWindow().getAttributes().flags & WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON) == 0, "completed task must release screen");
                activity.openEditor(901, "before", 0);
            });
            awaitUi(() -> {
                AlertDialog dialog = editor(activity);
                if (dialog == null) return false;
                EditText input = dialog.findViewById(android.R.id.edit);
                InputMethodManager ime = (InputMethodManager) activity.getSystemService(Activity.INPUT_METHOD_SERVICE);
                return input.hasWindowFocus() && ime.isActive(input);
            }, "dialog focus and default IME connection");
            onUi(() -> {
                AlertDialog dialog = editor(activity);
                check(dialog != null && dialog.isShowing(), "native editor dialog shown");
                EditText input = dialog.findViewById(android.R.id.edit);
                check(input.hasFocus(), "native editor focused");
                InputMethodManager ime = (InputMethodManager) activity.getSystemService(Activity.INPUT_METHOD_SERVICE);
                check(ime.isActive(input), "system default IME connected");
                input.setText("");
                InputConnection connection = input.onCreateInputConnection(new EditorInfo());
                check(connection != null, "real Android input connection");
                connection.setComposingText("zhong", 1);
                connection.commitText("中文🙂abc", 1);
                check(input.getText().toString().equals("中文🙂abc"), "Chinese and emoji composition committed");
                dialog.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            });
            awaitUi(() -> editor(activity) == null, "confirmation and JNI submission");
            awaitUi(activity::hasWindowFocus, "activity focus after IME dismissal");
            onUi(() -> {
                check(editor(activity) == null, "confirmation closes editor without JNI failure");
                check((activity.getWindow().getInsetsController().getSystemBarsAppearance()
                    & android.view.WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS) != 0,
                    "status bar icons remain readable after editor dismissal");
                activity.openEditor(902, "test-password", 1);
                EditText password = editor(activity).findViewById(android.R.id.edit);
                check((password.getInputType() & InputType.TYPE_MASK_VARIATION) == InputType.TYPE_TEXT_VARIATION_PASSWORD, "password editor mode");
                check(password.getTransformationMethod() != null, "password masked");
                editor(activity).cancel();
            });
            awaitUi(() -> editor(activity) == null, "password editor cancellation");
            onUi(() -> {
                activity.openEditor(903, "1.25", 3);
                EditText decimal = editor(activity).findViewById(android.R.id.edit);
                check((decimal.getInputType() & InputType.TYPE_NUMBER_FLAG_DECIMAL) != 0, "decimal keypad mode");
                editor(activity).cancel();
            });
            awaitUi(() -> editor(activity) == null, "decimal editor cancellation");
            java.io.File identityFile = new java.io.File(activity.getFilesDir(), "identity.json");
            check(identityFile.isFile() && identityFile.length() > 0, "Rust writes identity to Android private storage");
            onUi(activity::finish);
            awaitUi(activity::isDestroyed, "Activity destruction");
            check(true, "Activity finish returns without blocking Android main thread");
            MainActivity reopened = (MainActivity) startActivitySync(launch);
            awaitUi(reopened::hasWindowFocus, "reopened activity window focus");
            onUi(() -> {
                check(reopened != activity, "a new Activity can start in the same process");
                reopened.openEditor(904, "reopened", 0);
                check(editor(reopened) != null, "editor usable after reopening");
                editor(reopened).cancel();
            });
            awaitUi(() -> editor(reopened) == null, "reopened editor cancellation");
            ActivityMonitor monitor = addMonitor(MainActivity.class.getName(), null, false);
            onUi(reopened::recreate);
            MainActivity recreated = (MainActivity) monitor.waitForActivityWithTimeout(15000);
            removeMonitor(monitor);
            check(recreated != null && recreated != reopened, "Activity recreation completes");
            awaitUi(recreated::hasWindowFocus, "recreated activity window focus");
            onUi(() -> {
                recreated.openEditor(905, "recreated", 0);
                check(editor(recreated) != null, "editor usable after Activity recreation");
                editor(recreated).cancel();
            });
            report.putString("stream", "PASS: " + passed + " Android integration checks\n");
            finish(Activity.RESULT_OK, report);
        } catch (Throwable error) {
            report.putString("stream", "FAIL after " + passed + " checks: " + android.util.Log.getStackTraceString(error));
            finish(Activity.RESULT_CANCELED, report);
        }
    }
}
