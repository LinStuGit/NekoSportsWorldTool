package org.nekosportsworld.tool;

import android.app.AlertDialog;
import android.app.NativeActivity;
import android.os.Bundle;
import android.text.InputType;
import android.view.View;
import android.view.WindowInsets;
import android.view.WindowManager;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputMethodManager;
import android.widget.EditText;
import android.widget.FrameLayout;

/** Android UI thread owns the real text editor; Rust owns application state. */
public final class MainActivity extends NativeActivity {
    static { System.loadLibrary("nekosportsworldtool"); }
    private AlertDialog editorDialog;
    private AlertDialog deviceInfoDialog;
    private boolean taskActive;
    private boolean resumed;

    private native void nativeSubmitEdit(long id, String value);
    private native void nativeSetInsets(int left, int top, int right, int bottom);
    private native void nativeDeviceInfo(boolean initial, String info, String error);
    private native void nativeLocation(String city, double lat, double lon, String error);

    private static final int REQ_LOCATION = 4101;
    private android.location.LocationListener locationListener;
    private final android.os.Handler locationHandler = new android.os.Handler(android.os.Looper.getMainLooper());

    /** lite 版：请求一次本机定位（权限弹窗 → 最近/实时定位 → 城市逆地理编码）。 */
    public void requestLocation() {
        runOnUiThread(() -> {
            boolean fine = checkSelfPermission(android.Manifest.permission.ACCESS_FINE_LOCATION)
                == android.content.pm.PackageManager.PERMISSION_GRANTED;
            boolean coarse = checkSelfPermission(android.Manifest.permission.ACCESS_COARSE_LOCATION)
                == android.content.pm.PackageManager.PERMISSION_GRANTED;
            if (!fine && !coarse) {
                requestPermissions(new String[]{
                    android.Manifest.permission.ACCESS_FINE_LOCATION,
                    android.Manifest.permission.ACCESS_COARSE_LOCATION}, REQ_LOCATION);
                return;
            }
            readLocation();
        });
    }

    @Override
    public void onRequestPermissionsResult(int requestCode, String[] permissions, int[] grantResults) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults);
        if (requestCode != REQ_LOCATION) return;
        boolean granted = false;
        for (int result : grantResults) {
            if (result == android.content.pm.PackageManager.PERMISSION_GRANTED) granted = true;
        }
        if (granted) readLocation();
        else nativeLocation("", 0, 0, "未授予定位权限，无法自动读取城市和定位锚点");
    }

    private void readLocation() {
        try {
            android.location.LocationManager lm = (android.location.LocationManager) getSystemService(LOCATION_SERVICE);
            // 先用 10 分钟内的最近定位，避免每次都等 GPS 冷启动
            android.location.Location best = bestLastKnown(lm);
            if (best != null && System.currentTimeMillis() - best.getTime() < 10 * 60 * 1000L) {
                deliver(best);
                return;
            }
            if (locationListener != null) return; // 已在等待实时定位
            locationListener = new android.location.LocationListener() {
                @Override public void onLocationChanged(android.location.Location location) {
                    finishLocationWait(location);
                }
            };
            boolean any = false;
            for (String provider : lm.getProviders(true)) {
                if (android.location.LocationManager.GPS_PROVIDER.equals(provider)
                    || android.location.LocationManager.NETWORK_PROVIDER.equals(provider)) {
                    try {
                        lm.requestLocationUpdates(provider, 1000L, 1f, locationListener,
                            android.os.Looper.getMainLooper());
                        any = true;
                    } catch (Exception ignored) { }
                }
            }
            locationHandler.postDelayed(() -> finishLocationWait(null), 20000L);
            if (!any) finishLocationWait(null);
        } catch (Throwable error) {
            nativeLocation("", 0, 0, "定位失败: " + error);
        }
    }

    private void finishLocationWait(android.location.Location location) {
        if (locationListener == null) return;
        android.location.LocationManager lm = (android.location.LocationManager) getSystemService(LOCATION_SERVICE);
        lm.removeUpdates(locationListener);
        locationListener = null;
        locationHandler.removeCallbacksAndMessages(null);
        if (location != null) { deliver(location); return; }
        // 实时定位超时：退回最近一次历史定位
        android.location.Location best = bestLastKnown(lm);
        if (best != null) deliver(best);
        else nativeLocation("", 0, 0, "未能获取定位，请开启系统定位服务后重试");
    }

    private android.location.Location bestLastKnown(android.location.LocationManager lm) {
        android.location.Location best = null;
        for (String provider : lm.getProviders(true)) {
            android.location.Location fix = lm.getLastKnownLocation(provider);
            if (fix != null && (best == null || fix.getTime() > best.getTime())) best = fix;
        }
        return best;
    }

    /** 上报坐标；城市名经 Geocoder 逆地理编码（网络请求，放后台线程）。 */
    private void deliver(android.location.Location location) {
        final double lat = location.getLatitude();
        final double lon = location.getLongitude();
        new Thread(() -> {
            String city = null;
            try {
                android.location.Geocoder geocoder = new android.location.Geocoder(this, java.util.Locale.CHINA);
                java.util.List<android.location.Address> list = geocoder.getFromLocation(lat, lon, 1);
                if (list != null && !list.isEmpty()) {
                    android.location.Address address = list.get(0);
                    city = firstNonEmpty(address.getLocality(), address.getSubAdminArea(), address.getAdminArea());
                }
            } catch (Throwable ignored) { }
            final String resolved = city == null ? "" : city;
            runOnUiThread(() -> nativeLocation(resolved, lat, lon, ""));
        }).start();
    }

    private static String firstNonEmpty(String... values) {
        for (String value : values) {
            if (value != null && !value.trim().isEmpty()) return value;
        }
        return null;
    }

    public void requestDeviceInfo(boolean initial) {
        runOnUiThread(() -> {
            android.content.SharedPreferences preferences = getSharedPreferences("device_info", MODE_PRIVATE);
            if (isFinishing() || isDestroyed() || deviceInfoDialog != null) return;
            if (initial && preferences.getBoolean("asked", false)) return;
            String purpose = initial
                ? "允许后会保存到设备身份，用于后续登录和业务请求。"
                : "允许后只填写设备页，点击“保存”后用于后续登录和业务请求。";
            AlertDialog dialog = new AlertDialog.Builder(this)
                .setTitle("读取本机信息")
                .setMessage("是否允许读取本机品牌、型号和 Android 系统版本？\n\n" + purpose
                    + "品牌仅用于设备页展示。"
                    + "\n\n设备 UUID 继续沿用本应用首次生成并保存的值，不会更换。拒绝后仍可手动填写，也可在设备页再次读取。")
                .setPositiveButton("允许读取", (ignored, which) -> {
                    // First-run completion is acknowledged only after Rust saves it.
                    // If the Activity/process exits earlier, the next launch asks again.
                    if (!initial) preferences.edit().putBoolean("asked", true).apply();
                    try {
                        // These fields are read only after the user accepts this dialog.
                        org.json.JSONObject info = new org.json.JSONObject();
                        info.put("manufacturer", android.os.Build.MANUFACTURER);
                        info.put("model", android.os.Build.MODEL);
                        info.put("os_version", android.os.Build.VERSION.RELEASE);
                        nativeDeviceInfo(initial, info.toString(), "");
                    } catch (Exception error) {
                        nativeDeviceInfo(initial, "", "读取本机信息失败，请在设备页重试或手动填写");
                    }
                })
                .setNegativeButton("暂不读取", (ignored, which) -> {
                    preferences.edit().putBoolean("asked", true).apply();
                    nativeDeviceInfo(initial, "", "");
                })
                .create();
            dialog.setOnCancelListener(ignored -> {
                preferences.edit().putBoolean("asked", true).apply();
                nativeDeviceInfo(initial, "", "");
            });
            dialog.setOnDismissListener(ignored -> deviceInfoDialog = null);
            deviceInfoDialog = dialog;
            dialog.show();
        });
    }

    public void completeDeviceInfo() {
        getSharedPreferences("device_info", MODE_PRIVATE).edit().putBoolean("asked", true).apply();
    }

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        // NativeActivity's rendering surface must stay inside system bars.
        View content = findViewById(android.R.id.content);
        content.setOnApplyWindowInsetsListener((view, insets) -> {
            if (android.os.Build.VERSION.SDK_INT >= 30) {
                android.graphics.Insets bars = insets.getInsets(
                    WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
                nativeSetInsets(bars.left, bars.top, bars.right, bars.bottom);
            } else {
                nativeSetInsets(insets.getSystemWindowInsetLeft(), insets.getSystemWindowInsetTop(),
                    insets.getSystemWindowInsetRight(), insets.getSystemWindowInsetBottom());
            }
            return insets;
        });
        content.requestApplyInsets();
        applySystemBarStyle();
    }

    @Override public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (hasFocus) applySystemBarStyle();
    }

    private void applySystemBarStyle() {
        if (android.os.Build.VERSION.SDK_INT >= 30) {
            getWindow().getInsetsController().setSystemBarsAppearance(
                android.view.WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS |
                android.view.WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS,
                android.view.WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS |
                android.view.WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS);
        } else {
            getWindow().getDecorView().setSystemUiVisibility(
                View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR | View.SYSTEM_UI_FLAG_LIGHT_NAVIGATION_BAR);
        }
    }

    public void openEditor(long id, String initialValue, int kind) {
        runOnUiThread(() -> {
            if (isFinishing() || isDestroyed() || editorDialog != null) return;
            EditText input = new EditText(this);
            input.setId(android.R.id.edit);
            int inputType;
            switch (kind) {
                case 1: inputType = InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD; break;
                case 2: inputType = InputType.TYPE_CLASS_NUMBER | InputType.TYPE_NUMBER_FLAG_SIGNED; break;
                case 3: inputType = InputType.TYPE_CLASS_NUMBER | InputType.TYPE_NUMBER_FLAG_SIGNED | InputType.TYPE_NUMBER_FLAG_DECIMAL; break;
                default: inputType = InputType.TYPE_CLASS_TEXT;
            }
            input.setInputType(inputType);
            input.setSingleLine(true);
            input.setImeOptions(EditorInfo.IME_ACTION_DONE | EditorInfo.IME_FLAG_NO_EXTRACT_UI);
            input.setText(initialValue);
            input.selectAll();
            int margin = Math.round(20 * getResources().getDisplayMetrics().density);
            FrameLayout container = new FrameLayout(this);
            container.setPadding(margin, 0, margin, 0);
            container.addView(input, new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT, FrameLayout.LayoutParams.WRAP_CONTENT));
            AlertDialog dialog = new AlertDialog.Builder(this)
                .setTitle(kind == 1 ? "输入密码" : "编辑内容")
                .setView(container)
                .setPositiveButton("确定", (ignored, which) -> nativeSubmitEdit(id, input.getText().toString()))
                .setNegativeButton("取消", null)
                .create();
            editorDialog = dialog;
            dialog.setOnDismissListener(ignored -> {
                InputMethodManager ime = (InputMethodManager) getSystemService(INPUT_METHOD_SERVICE);
                ime.hideSoftInputFromWindow(input.getWindowToken(), 0);
                editorDialog = null;
            });
            input.setOnEditorActionListener((view, action, event) -> {
                if (action != EditorInfo.IME_ACTION_DONE) return false;
                nativeSubmitEdit(id, input.getText().toString());
                dialog.dismiss();
                return true;
            });
            dialog.setOnShowListener(ignored -> {
                input.requestFocus();
                dialog.getWindow().setSoftInputMode(
                    WindowManager.LayoutParams.SOFT_INPUT_STATE_ALWAYS_VISIBLE |
                    WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE);
                input.post(() -> ((InputMethodManager) getSystemService(INPUT_METHOD_SERVICE))
                    .showSoftInput(input, InputMethodManager.SHOW_IMPLICIT));
            });
            dialog.show();
        });
    }

    public void setTaskActive(boolean active) {
        runOnUiThread(() -> {
            taskActive = active;
            updateScreenPolicy();
        });
    }

    public void copyText(String text) {
        runOnUiThread(() -> {
            android.content.ClipboardManager clipboard =
                (android.content.ClipboardManager) getSystemService(CLIPBOARD_SERVICE);
            clipboard.setPrimaryClip(android.content.ClipData.newPlainText("NekoSportsWorldTool", text));
        });
    }

    /** 自更新：把私有目录里已下载的 update.apk 交给系统安装器。 */
    public void installApk(String path) {
        runOnUiThread(() -> {
            try {
                if (path == null || !new java.io.File(path).isFile()) {
                    android.widget.Toast.makeText(this,
                        "安装包不存在，请重新检查更新",
                        android.widget.Toast.LENGTH_LONG).show();
                    return;
                }
                android.content.Intent intent = new android.content.Intent(android.content.Intent.ACTION_VIEW);
                intent.addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK);
                intent.addFlags(android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION);
                intent.setDataAndType(ApkProvider.uriForUpdate(), "application/vnd.android.package-archive");
                startActivity(intent);
            } catch (Exception error) {
                android.widget.Toast.makeText(this,
                    "无法启动安装器，请到系统设置允许本应用安装未知应用后重试",
                    android.widget.Toast.LENGTH_LONG).show();
            }
        });
    }

    /** 当前 APK 的 versionName（与 Release tag 对齐，供更新检查比较）。 */
    public String appVersionName() {
        try {
            return getPackageManager().getPackageInfo(getPackageName(), 0).versionName;
        } catch (Exception error) {
            return "";
        }
    }

    private void updateScreenPolicy() {
        if (taskActive && resumed) getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        else getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
    }

    @Override protected void onResume() {
        super.onResume();
        resumed = true;
        updateScreenPolicy();
    }

    @Override protected void onPause() {
        resumed = false;
        updateScreenPolicy();
        super.onPause();
    }

    @Override protected void onDestroy() {
        if (editorDialog != null) editorDialog.dismiss();
        if (deviceInfoDialog != null) deviceInfoDialog.dismiss();
        if (locationListener != null) {
            try {
                android.location.LocationManager lm =
                    (android.location.LocationManager) getSystemService(LOCATION_SERVICE);
                lm.removeUpdates(locationListener);
            } catch (Throwable ignored) { }
            locationListener = null;
            locationHandler.removeCallbacksAndMessages(null);
        }
        taskActive = false;
        updateScreenPolicy();
        super.onDestroy();
    }
}
