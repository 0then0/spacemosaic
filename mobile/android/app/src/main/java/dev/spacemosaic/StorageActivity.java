package dev.spacemosaic;

import android.app.NativeActivity;
import android.app.AlertDialog;
import android.os.Bundle;
import android.os.CancellationSignal;
import android.os.Handler;
import android.os.Looper;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.provider.DocumentsContract;
import android.view.Gravity;
import android.view.ViewGroup;
import android.widget.*;
import org.json.JSONArray;
import org.json.JSONObject;
import java.io.*;
import java.lang.ref.WeakReference;
import java.nio.charset.StandardCharsets;
import java.util.Locale;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/** Native metadata cursors and system dialogs; Rust owns recursive traversal. */
public final class StorageActivity extends NativeActivity {
    static { System.loadLibrary("spacemosaic"); }
    private static final int PICK_TREE = 43, SAVE_EXPORT = 44;
    private static final int PAGE_SIZE = 32, MAX_BYTES = 256 * 1024;
    private static native boolean deliver(String json);
    private static native long begin();
    private static native boolean isCurrent(long generation);
    private static native String resultPage(String request);
    private static native void exportDone(long id, int outcome);
    private static native boolean back();
    private static native void setInsets(int left, int top, int right, int bottom,
        int x, int y, int width, int height, int windowWidth, int windowHeight, int footerTop, boolean systemDark);
    private android.view.WindowInsets lastInsets;
    private Object backCallback;
    private static WeakReference<StorageActivity> current = new WeakReference<>(null);
    private final ExecutorService worker = Executors.newSingleThreadExecutor();
    private final Handler handler = new Handler(Looper.getMainLooper());
    private long pickerGeneration, exportId;
    private volatile long cursorGeneration, cursorScan;
    private long cancelledThrough;
    private final Object cursorLock = new Object();
    private int idColumn, nameColumn, mimeColumn, sizeColumn;
    private boolean pickerOpen, restored, browserLoading, browserAgain, allocated;
    private volatile boolean destroyed;
    private volatile CancellationSignal cancellation;
    // Only the single Rust scanner worker opens, reads, and closes this cursor.
    private Cursor cursor;
    private Uri cursorTree;
    private File exportSource;
    private JSONObject catalog = new JSONObject(), lastMessage;
    private LinearLayout browserRows;
    private AlertDialog browserDialog;
    private Button launcher;
    private EditText search;
    private long directory, parent;
    private int offset, nextOffset;
    private String mode = "directory", sort = "size", language = "en", browserQuery = "";
    private final java.util.ArrayDeque<Long> directories = new java.util.ArrayDeque<>();
    private final java.util.ArrayDeque<Integer> offsets = new java.util.ArrayDeque<>();
    private int appearance;
    private long browserRequest, lastPageGeneration;

    private int themeResource() {
        boolean dark = appearance == 2 || (appearance == 0 &&
            (getResources().getConfiguration().uiMode & android.content.res.Configuration.UI_MODE_NIGHT_MASK)
                == android.content.res.Configuration.UI_MODE_NIGHT_YES);
        return dark ? android.R.style.Theme_Material_NoActionBar : android.R.style.Theme_Material_Light_NoActionBar;
    }
    private android.content.Context viewContext() { return new android.view.ContextThemeWrapper(this, themeResource()); }
    private String text(String key) { return catalog.optString(key, key); }
    private JSONObject readCatalog(String tag) throws Exception {
        try (InputStream stream = getAssets().open(tag + ".json"); ByteArrayOutputStream output = new ByteArrayOutputStream()) {
            byte[] bytes = new byte[4096]; int count;
            while ((count = stream.read(bytes)) != -1) output.write(bytes, 0, count);
            return new JSONObject(output.toString("UTF-8"));
        }
    }
    public void configureAppearance(int preference, String tag) {
        runOnUiThread(() -> {
            boolean changed = appearance != preference || !language.equals(tag);
            appearance = preference; language = tag;
            applySystemAppearance();
            try { catalog = readCatalog(tag); } catch (Exception error) { /* Keep the last complete catalog. */ }
            if (launcher != null) launcher.setText(text("accessible_browser"));
            if (changed && browserDialog != null) { browserDialog.dismiss(); showBrowser(); }
            if (!restored) {
                restored = true;
                String retained = getPreferences(MODE_PRIVATE).getString("tree", null);
                if (retained != null) {
                    Uri tree = Uri.parse(retained);
                    boolean granted = false;
                    for (android.content.UriPermission permission : getContentResolver().getPersistedUriPermissions()) {
                        if (permission.getUri().equals(tree) && permission.isReadPermission()) granted = true;
                    }
                    long generation = begin();
                    if (granted) ready(generation, tree, true);
                    else send(generation, "denied", null, false, "access_revoked");
                }
            }
        });
    }
    @Override public void onCreate(Bundle state) {
        setTheme(themeResource()); super.onCreate(state);
        current = new WeakReference<>(this);
        String tag = Locale.getDefault().getLanguage();
        language = tag.equals("pt") ? "pt-BR" : tag;
        try { catalog = readCatalog(language); }
        catch (Exception error) { try { catalog = readCatalog("en"); } catch (Exception ignored) { /* Missing assets remain explicit keys. */ } }
        launcher = new Button(viewContext()); launcher.setText(text("accessible_browser"));
        launcher.setOnClickListener(view -> showBrowser());
        FrameLayout.LayoutParams params = new FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT,
            (int)(64 * getResources().getDisplayMetrics().density), Gravity.BOTTOM);
        launcher.setOnApplyWindowInsetsListener((view, insets) -> {
            int bottom = android.os.Build.VERSION.SDK_INT >= 30
                ? insets.getInsets(android.view.WindowInsets.Type.systemBars() | android.view.WindowInsets.Type.displayCutout()).bottom
                : insets.getSystemWindowInsetBottom();
            view.setPadding(view.getPaddingLeft(), view.getPaddingTop(), view.getPaddingRight(), bottom);
            ViewGroup.LayoutParams layout = view.getLayoutParams();
            layout.height = (int)(64 * getResources().getDisplayMetrics().density) + bottom;
            view.setLayoutParams(layout);
            return insets;
        });
        addContentView(launcher, params);
        android.view.View content = findViewById(android.R.id.content);
        content.setOnApplyWindowInsetsListener((view, insets) -> {
            lastInsets = insets;
            view.post(() -> reportInsets(content));
            return insets;
        });
        content.addOnLayoutChangeListener((view,l,t,r,b,ol,ot,oright,ob) -> reportInsets(content));
        launcher.addOnLayoutChangeListener((view,l,t,r,b,ol,ot,oright,ob) -> reportInsets(content));
        content.requestApplyInsets();
        content.post(this::applySystemAppearance);
        if (android.os.Build.VERSION.SDK_INT >= 33) {
            backCallback = BackCallbacks.register(this);
        }
    }
    // Isolate API 33 callback types so Android 8-12 can load this Activity.
    private static final class BackCallbacks {
        static Object register(StorageActivity activity) {
            android.window.OnBackInvokedCallback callback = () -> { if (!back()) activity.finish(); };
            activity.getOnBackInvokedDispatcher().registerOnBackInvokedCallback(
                android.window.OnBackInvokedDispatcher.PRIORITY_DEFAULT, callback);
            return callback;
        }
        static void unregister(StorageActivity activity, Object callback) {
            activity.getOnBackInvokedDispatcher().unregisterOnBackInvokedCallback(
                (android.window.OnBackInvokedCallback)callback);
        }
    }
    private void reportInsets(android.view.View content) {
        if (lastInsets == null || content.getWidth() == 0 || content.getHeight() == 0 || launcher.getHeight() == 0) return;
        int left,top,right,bottom;
        int windowWidth = getWindow().getDecorView().getWidth();
        int windowHeight = getWindow().getDecorView().getHeight();
        if (android.os.Build.VERSION.SDK_INT >= 30) {
            android.graphics.Insets bars = lastInsets.getInsets(android.view.WindowInsets.Type.systemBars()
                | android.view.WindowInsets.Type.displayCutout() | android.view.WindowInsets.Type.ime());
            left = bars.left; top = bars.top; right = bars.right; bottom = bars.bottom;
            android.graphics.Rect bounds = getWindowManager().getCurrentWindowMetrics().getBounds();
            windowWidth = bounds.width(); windowHeight = bounds.height();
        } else {
            left = lastInsets.getSystemWindowInsetLeft(); top = lastInsets.getSystemWindowInsetTop();
            right = lastInsets.getSystemWindowInsetRight(); bottom = lastInsets.getSystemWindowInsetBottom();
        }
        int[] location = new int[2]; content.getLocationInWindow(location);
        int[] footer = new int[2]; launcher.getLocationInWindow(footer);
        setInsets(left,top,right,bottom,location[0],location[1],content.getWidth(),content.getHeight(),
            windowWidth,windowHeight,footer[1],
            (getResources().getConfiguration().uiMode & android.content.res.Configuration.UI_MODE_NIGHT_MASK)
                == android.content.res.Configuration.UI_MODE_NIGHT_YES);
    }
    private void applySystemAppearance() {
        boolean dark = appearance == 2 || (appearance == 0 &&
            (getResources().getConfiguration().uiMode & android.content.res.Configuration.UI_MODE_NIGHT_MASK)
                == android.content.res.Configuration.UI_MODE_NIGHT_YES);
        if (launcher != null) {
            launcher.setTextColor(dark ? 0xFFFAF9F6 : 0xFF20201F);
            launcher.setBackgroundTintList(android.content.res.ColorStateList.valueOf(dark ? 0xFF353534 : 0xFFE4E3DF));
        }
        if (android.os.Build.VERSION.SDK_INT >= 30) {
            android.view.WindowInsetsController controller = getWindow().getInsetsController();
            if (controller != null) {
                int mask = android.view.WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS
                    | android.view.WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS;
                controller.setSystemBarsAppearance(dark ? 0 : mask, mask);
            }
        } else {
            android.view.View decor = getWindow().getDecorView();
            int mask = android.view.View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR | android.view.View.SYSTEM_UI_FLAG_LIGHT_NAVIGATION_BAR;
            decor.setSystemUiVisibility(dark ? decor.getSystemUiVisibility() & ~mask : decor.getSystemUiVisibility() | mask);
        }
    }
    private Button button(String key, Runnable action) {
        Button button = new Button(viewContext()); button.setText(text(key)); button.setMinHeight((int)(48 * getResources().getDisplayMetrics().density));
        button.setOnClickListener(view -> action.run()); return button;
    }
    private void command(String name) {
        if (name.equals("rescan")) {
            directory = 0; directories.clear(); offset = 0; offsets.clear(); mode = "directory";
            browserQuery = ""; if (search != null) search.setText("");
        }
        browserRequest++;
        try { resultPage(new JSONObject().put("command", name).toString()); }
        catch (Exception ignored) { /* A later refresh still exposes the current result. */ }
        handler.postDelayed(this::refreshBrowser, 250);
    }
    private void showBrowser() {
        if (destroyed || browserDialog != null) return;
        LinearLayout content = new LinearLayout(viewContext()); content.setOrientation(LinearLayout.VERTICAL);
        LinearLayout actions = new LinearLayout(viewContext());
        actions.addView(button("choose", () -> pickTree(begin())));
        actions.addView(button("rescan", () -> command("rescan")));
        actions.addView(button("cancel", () -> command("cancel")));
        HorizontalScrollView actionScroll = new HorizontalScrollView(viewContext()); actionScroll.addView(actions); content.addView(actionScroll);
        LinearLayout modes = new LinearLayout(viewContext());
        for (String key : new String[]{"directory", "largest", "extensions"}) {
            modes.addView(button(key, () -> { mode = key; offset = 0; offsets.clear(); refreshBrowser(); }));
        }
        modes.addView(button("allocated", () -> { allocated = !allocated; setting("allocated",allocated ? 1 : 0); refreshBrowser(); }));
        modes.addView(button("settings", this::settings));
        modes.addView(button("sort", () -> {
            sort = sort.equals("size") ? "name" : "size"; offset = 0; offsets.clear(); refreshBrowser();
        }));
        HorizontalScrollView modeScroll = new HorizontalScrollView(viewContext()); modeScroll.addView(modes); content.addView(modeScroll);
        search = new EditText(viewContext()); search.setSingleLine(true); search.setHint(text("search_hint"));
        search.setContentDescription(text("search")); search.setText(browserQuery); content.addView(search);
        search.addTextChangedListener(new android.text.TextWatcher() {
            public void beforeTextChanged(CharSequence s, int a, int c, int f) {}
            public void onTextChanged(CharSequence s, int a, int b, int c) { browserQuery = s.toString(); offset = 0; offsets.clear(); refreshBrowser(); }
            public void afterTextChanged(android.text.Editable e) {}
        });
        ScrollView scroll = new ScrollView(viewContext()); browserRows = new LinearLayout(viewContext());
        browserRows.setOrientation(LinearLayout.VERTICAL); scroll.addView(browserRows);
        content.addView(scroll, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1));
        LinearLayout exports = new LinearLayout(viewContext());
        exports.addView(button("export_json", () -> command("json"))); exports.addView(button("export_csv", () -> command("csv")));
        HorizontalScrollView exportScroll = new HorizontalScrollView(viewContext()); exportScroll.addView(exports); content.addView(exportScroll);
        browserDialog = new AlertDialog.Builder(viewContext()).setTitle(text("accessible_browser"))
            .setView(content).setNegativeButton(text("close"), (dialog, which) -> {}).create();
        browserDialog.setOnKeyListener((dialog, code, event) -> {
            if (code == android.view.KeyEvent.KEYCODE_BACK && event.getAction() == android.view.KeyEvent.ACTION_UP && directory != 0) {
                navigate(directories.isEmpty() ? parent : directories.removeLast(), false); return true;
            }
            return false;
        });
        browserDialog.setOnDismissListener(dialog -> { browserDialog = null; browserRows = null; search = null; browserRequest++; });
        browserDialog.show();
        browserDialog.getWindow().setLayout(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT);
        refreshBrowser();
    }
    private void navigate(long index) { navigate(index,true); }
    private void navigate(long index, boolean remember) {
        if (remember) directories.addLast(directory);
        directory = index; offset = 0; offsets.clear(); mode = "directory";
        if (search != null) search.setText("");
        try { resultPage(new JSONObject().put("command", "navigate").put("index", index).put("generation", lastPageGeneration).toString()); }
        catch (Exception ignored) { /* The result reader bounds-checks the directory again. */ }
        refreshBrowser();
    }
    private void refreshBrowser() {
        if (browserRows == null || destroyed) return;
        browserRequest++;
        if (browserLoading) { browserAgain = true; return; }
        browserLoading = true; browserAgain = false;
        final long sequence = browserRequest;
        final JSONObject request = new JSONObject();
        try {
            request.put("directory", directory).put("offset", offset).put("mode", mode).put("sort", sort)
                .put("query", search == null ? "" : search.getText().toString()).put("language", language).put("allocated",allocated);
        } catch (Exception error) { browserLoading = false; return; }
        worker.execute(() -> {
            JSONObject response;
            try { response = new JSONObject(resultPage(request.toString())); }
            catch (Exception error) { response = new JSONObject(); }
            final JSONObject page = response;
            runOnUiThread(() -> {
                browserLoading = false;
                if (browserRows != null && sequence == browserRequest) renderPage(page);
                if (browserAgain || sequence != browserRequest) refreshBrowser();
            });
        });
    }
    private void addText(String value) {
        TextView label = new TextView(viewContext()); label.setText(value); label.setTextSize(16);
        int padding = (int)(10 * getResources().getDisplayMetrics().density);
        label.setPadding(padding, padding, padding, padding); browserRows.addView(label);
    }
    private void renderPage(JSONObject page) {
        if (page.has("generation") && !isCurrent(page.optLong("generation"))) { handler.postDelayed(this::refreshBrowser, 100); return; }
        lastPageGeneration = page.optLong("generation");
        browserRows.removeAllViews();
        String status = page.optString("status", "unavailable");
        if (status.equals("waiting") && lastMessage != null) {
            String pickerStatus = lastMessage.optString("status");
            if (!pickerStatus.equals("ready")) status = pickerStatus;
        }
        addText(text("mobile_scope")); addText(text("size_caveat")); addText(text(status)); addText(page.optString("summary"));
        if (!page.isNull("error")) addText(text(page.optString("error")));
        if (page.has("name")) addText(page.optString("name"));
        parent = page.optLong("parent", 0);
        if (directory != 0) browserRows.addView(button("back", () -> navigate(directories.isEmpty() ? parent : directories.removeLast(), false)));
        JSONArray entries = page.optJSONArray("entries");
        if (entries != null) for (int index = 0; index < entries.length(); index++) {
            JSONObject entry = entries.optJSONObject(index); if (entry == null) continue;
            if (!entry.has("id")) { addText(entry.optString("name") + "\n" + entry.optString("size_label") + "\n" + entry.optString("count_label")); continue; }
            Button row = new Button(viewContext()); row.setAllCaps(false); row.setGravity(Gravity.START | Gravity.CENTER_VERTICAL);
            row.setMinHeight((int)(64 * getResources().getDisplayMetrics().density));
            row.setText(text(entry.optBoolean("directory") ? "directory" : "file") + ": " + entry.optString("name") + "\n" + entry.optString("size_label") + (entry.optString("scan_note").isEmpty() ? "" : "\n" + entry.optString("scan_note")));
            row.setOnClickListener(view -> {
                if (entry.optBoolean("directory")) navigate(entry.optLong("id"));
                else details(entry);
            }); browserRows.addView(row);
        }
        nextOffset = page.optInt("next", offset);
        if (offset > 0) browserRows.addView(button("previous_page", () -> { offset = offsets.isEmpty() ? 0 : offsets.removeLast(); refreshBrowser(); }));
        if (page.optBoolean("has_more")) browserRows.addView(button("next_page", () -> { offsets.addLast(offset); offset = nextOffset; refreshBrowser(); }));
        browserRows.addView(button("refresh_result", this::refreshBrowser));
        if (status.equals("scanning") || (status.equals("waiting") && !pickerOpen && lastMessage != null && lastMessage.optString("status").equals("ready"))) handler.postDelayed(this::refreshBrowser, 500);
    }
    private void setting(String command, int index) {
        try { resultPage(new JSONObject().put("command", command).put("index",index).toString()); }
        catch (Exception ignored) { /* The existing setting remains visible. */ }
    }
    private void settings() {
        new AlertDialog.Builder(viewContext()).setTitle(text("settings"))
            .setItems(new String[]{text("appearance"), text("language")}, (dialog,which) -> {
                if (which == 0) new AlertDialog.Builder(viewContext()).setTitle(text("appearance"))
                    .setSingleChoiceItems(new String[]{text("system"),text("light"),text("dark")},appearance,(choice,index) -> { setting("appearance",index); choice.dismiss(); }).show();
                else new AlertDialog.Builder(viewContext()).setTitle(text("language"))
                    .setItems(new String[]{text("system"),"en","ru","es","de","fr","ja","pt-BR"},(choice,index) -> setting("language",index == 0 ? 7 : index-1)).show();
            }).show();
    }
    @Override public void onConfigurationChanged(android.content.res.Configuration configuration) {
        super.onConfigurationChanged(configuration);
        applySystemAppearance();
        reportInsets(findViewById(android.R.id.content));
        if (appearance == 0 && browserDialog != null) { browserDialog.dismiss(); showBrowser(); }
    }
    private void details(JSONObject entry) {
        try { resultPage(new JSONObject().put("command", "select").put("index", entry.optLong("id")).put("generation", lastPageGeneration).toString()); }
        catch (Exception ignored) { /* Native details still describe the immutable result row. */ }
        String locator = entry.optString("locator");
        String details = text("logical_bytes") + ": " + entry.optString("logical_label") + "\n"
            + text("allocated") + ": " + entry.optString("allocated_label") + "\n" + locator;
        if (entry.optBoolean("hard_link")) details += "\n" + text("hard_link_note");
        if (!entry.isNull("error")) details += "\n" + text(entry.optString("error"));
        new AlertDialog.Builder(viewContext()).setTitle(entry.optString("name")).setMessage(details)
            .setPositiveButton(text("reveal"), (d, w) -> view(locator))
            .setNeutralButton(text("copy_path"), (d, w) -> copy(locator))
            .setNegativeButton(text("close"), (d, w) -> {}).show();
    }
    private void copy(String locator) {
        android.content.ClipboardManager clipboard = (android.content.ClipboardManager)getSystemService(CLIPBOARD_SERVICE);
        clipboard.setPrimaryClip(android.content.ClipData.newPlainText(text("copy_path"), locator));
    }
    private void view(String locator) {
        try { startActivity(new Intent(Intent.ACTION_VIEW).setDataAndType(Uri.parse(locator), "*/*")
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)); }
        catch (RuntimeException error) { Toast.makeText(this, text("open_failed"), Toast.LENGTH_LONG).show(); }
    }
    public void pickTree(long generation) {
        runOnUiThread(() -> {
            if (pickerOpen) { send(generation, "unavailable", null, false, "picker_busy"); return; }
            CancellationSignal signal = cancellation; if (signal != null) signal.cancel();
            directory = 0; directories.clear(); offset = 0; offsets.clear(); browserQuery = ""; mode = "directory"; if (search != null) search.setText(""); lastMessage = null;
            pickerGeneration = generation; pickerOpen = true;
            Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE)
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION);
            try { startActivityForResult(intent, PICK_TREE); }
            catch (RuntimeException error) { pickerOpen = false; send(generation, "unavailable", null, false, "picker_unavailable"); }
        });
    }
    private void ready(long generation, Uri tree, boolean persisted) {
        Uri root = DocumentsContract.buildDocumentUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree));
        send(generation, "ready", root.toString(), persisted, null);
    }
    @Override protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        if (request == SAVE_EXPORT) {
            final File source = exportSource; final long id = exportId;
            exportSource = null;
            if (result != RESULT_OK || data == null || data.getData() == null || source == null) { exportDone(id, result == RESULT_CANCELED ? 2 : 0); return; }
            final Uri destination = data.getData();
            worker.execute(() -> {
                boolean success = false;
                try (InputStream input = new FileInputStream(source); OutputStream output = getContentResolver().openOutputStream(destination, "wt")) {
                    if (output == null) throw new IOException("destination_unavailable");
                    byte[] bytes = new byte[64 * 1024]; int count;
                    while ((count = input.read(bytes)) != -1) output.write(bytes, 0, count);
                    output.flush();
                    success = true;
                } catch (Exception error) { success = false; }
                exportDone(id, success ? 1 : 0);
            }); return;
        }
        if (request != PICK_TREE) return;
        pickerOpen = false;
        if (result != RESULT_OK || data == null || data.getData() == null) { send(pickerGeneration, "cancelled", null, false, null); return; }
        Uri tree = data.getData(); boolean persisted = false;
        if ((data.getFlags() & Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION) != 0) {
            try { getContentResolver().takePersistableUriPermission(tree, data.getFlags() & Intent.FLAG_GRANT_READ_URI_PERMISSION); persisted = true; }
            catch (SecurityException error) { /* Continue using the temporary read grant. */ }
        }
        getPreferences(MODE_PRIVATE).edit().remove("tree").apply();
        if (persisted) getPreferences(MODE_PRIVATE).edit().putString("tree", tree.toString()).apply();
        ready(pickerGeneration, tree, persisted);
    }
    private void send(long generation, String status, String scope, boolean persisted, String error) {
        try {
            JSONObject value = new JSONObject().put("generation", generation).put("status", status)
                .put("scope", scope == null ? JSONObject.NULL : scope).put("entries", new JSONArray())
                .put("truncated", false).put("persisted", persisted).put("error", error == null ? JSONObject.NULL : error);
            if (deliver(value.toString())) runOnUiThread(() -> { if (isCurrent(generation)) { lastMessage = value; refreshBrowser(); } });
        } catch (Exception errorEncoding) { throw new IllegalStateException("bridge_encoding", errorEncoding); }
    }
    private static JSONObject page(JSONArray entries, boolean done, String error) throws Exception {
        return new JSONObject().put("entries", entries).put("done", done).put("error", error == null ? JSONObject.NULL : error);
    }
    private void closeCursor(long generation, long scan) {
        if (cursorGeneration == generation && cursorScan == scan) { if (cursor != null) cursor.close(); cursor = null; cancellation = null; }
    }
    /** Called on the Rust scanner worker, except cancellation and main-queue dialog handoffs. */
    public String provider(String encoded) {
        try {
            JSONObject request = new JSONObject(encoded); String op = request.getString("op");
            long generation = request.optLong("generation"), scan = request.optLong("scan");
            if (op.equals("cache")) return new JSONObject().put("path", new File(getCacheDir(),
                "spacemosaic-export-" + request.getLong("id") + (request.getBoolean("json") ? ".json" : ".csv")).getAbsolutePath()).toString();
            if (op.equals("save")) {
                long id = request.getLong("id"); boolean json = request.getBoolean("json");
                File source = new File(request.getString("path"));
                File expected = new File(getCacheDir(), "spacemosaic-export-" + id + (json ? ".json" : ".csv"));
                if (!source.getCanonicalFile().equals(expected.getCanonicalFile())) return "{}";
                runOnUiThread(() -> {
                    if (destroyed || exportSource != null || pickerOpen) { exportDone(id, 0); return; }
                    exportSource = source; exportId = id;
                    Intent intent = new Intent(Intent.ACTION_CREATE_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE)
                        .setType(json ? "application/json" : "text/csv").putExtra(Intent.EXTRA_TITLE, "spacemosaic." + (json ? "json" : "csv"));
                    try { startActivityForResult(intent, SAVE_EXPORT); }
                    catch (RuntimeException error) { exportSource = null; exportDone(id, 0); }
                });
                return new JSONObject().put("accepted", true).toString();
            }
            if (op.equals("copy") || op.equals("view")) {
                String locator = request.getString("locator");
                runOnUiThread(() -> { if (op.equals("copy")) copy(locator); else view(locator); }); return "{}";
            }
            if (op.equals("cancel")) {
                CancellationSignal signal;
                synchronized (cursorLock) {
                    // IDs increase across rescans. Record cancellation even if
                    // the worker has not registered its cursor yet.
                    cancelledThrough = Math.max(cancelledThrough, scan);
                    signal = cursorGeneration == generation && cursorScan == scan ? cancellation : null;
                }
                if (signal != null) signal.cancel();
                return page(new JSONArray(), true, null).toString();
            }
            if (op.equals("close") || op.equals("finish")) { closeCursor(generation, scan); return page(new JSONArray(), true, null).toString(); }
            if (!isCurrent(generation) || destroyed) return page(new JSONArray(), true, "cancelled").toString();
            if (op.equals("open")) {
                closeCursor(cursorGeneration, cursorScan);
                synchronized (cursorLock) {
                    if (scan <= cancelledThrough) throw new android.os.OperationCanceledException();
                    cursorGeneration = generation; cursorScan = scan; cancellation = new CancellationSignal();
                }
                cursorTree = Uri.parse(request.getString("locator"));
                String[] columns = {DocumentsContract.Document.COLUMN_DOCUMENT_ID, DocumentsContract.Document.COLUMN_DISPLAY_NAME,
                    DocumentsContract.Document.COLUMN_MIME_TYPE, DocumentsContract.Document.COLUMN_SIZE};
                Uri children = DocumentsContract.buildChildDocumentsUriUsingTree(cursorTree, DocumentsContract.getDocumentId(cursorTree));
                cursor = getContentResolver().query(children, columns, null, null, null, cancellation);
                if (cursor == null) throw new IOException("provider_unavailable");
                idColumn = cursor.getColumnIndex(DocumentsContract.Document.COLUMN_DOCUMENT_ID);
                nameColumn = cursor.getColumnIndex(DocumentsContract.Document.COLUMN_DISPLAY_NAME);
                mimeColumn = cursor.getColumnIndex(DocumentsContract.Document.COLUMN_MIME_TYPE);
                sizeColumn = cursor.getColumnIndex(DocumentsContract.Document.COLUMN_SIZE);
                if (idColumn < 0) throw new IOException("missing_document_id");
                return page(new JSONArray(), false, null).toString();
            }
            if (!op.equals("next") || cursor == null || cursorGeneration != generation || cursorScan != scan) throw new IOException("missing_cursor");
            JSONArray entries = new JSONArray(); boolean done = false; String error = null;
            while (entries.length() < PAGE_SIZE) {
                cancellation.throwIfCanceled();
                if (!cursor.moveToNext()) { done = true; break; }
                String id = cursor.getString(idColumn);
                String mime = mimeColumn < 0 || cursor.isNull(mimeColumn) ? null : cursor.getString(mimeColumn);
                boolean directory = DocumentsContract.Document.MIME_TYPE_DIR.equals(mime);
                if (id == null) throw new IOException("missing_document_id");
                entries.put(new JSONObject().put("name", nameColumn < 0 || cursor.isNull(nameColumn) ? id : cursor.getString(nameColumn))
                    .put("locator", DocumentsContract.buildDocumentUriUsingTree(cursorTree, id).toString())
                    .put("kind", mime == null ? "other" : (directory ? "directory" : "file"))
                    .put("error", mime == null ? "metadata_unavailable" : JSONObject.NULL)
                    .put("logical_bytes", directory || mime == null || sizeColumn < 0 || cursor.isNull(sizeColumn) || cursor.getLong(sizeColumn) < 0 ? JSONObject.NULL : cursor.getLong(sizeColumn))
                    .put("allocated_bytes", JSONObject.NULL));
                if (entries.toString().getBytes(StandardCharsets.UTF_8).length > MAX_BYTES - 1024) {
                    entries.remove(entries.length() - 1); done = true; error = "message_too_large"; break;
                }
            }
            if (done && (cursor.getExtras().getBoolean(DocumentsContract.EXTRA_LOADING, false)
                || cursor.getExtras().getString(DocumentsContract.EXTRA_ERROR) != null)) error = "provider_unavailable";
            return page(entries, done, error).toString();
        } catch (SecurityException error) { return failure("access_revoked"); }
        catch (android.os.OperationCanceledException error) { return failure("cancelled"); }
        catch (Exception error) { return failure("provider_unavailable"); }
    }
    private String failure(String code) {
        try { return page(new JSONArray(), true, code).toString(); } catch (Exception impossible) { return "{}"; }
    }
    @Override public void onBackPressed() { if (!back()) super.onBackPressed(); }
    @Override protected void onDestroy() {
        destroyed = true;
        if (android.os.Build.VERSION.SDK_INT >= 33 && backCallback != null)
            BackCallbacks.unregister(this, backCallback);
        CancellationSignal signal = cancellation; if (signal != null) signal.cancel();
        handler.removeCallbacksAndMessages(null);
        if (browserDialog != null) browserDialog.dismiss();
        if (exportSource != null) exportDone(exportId, 0);
        if (current.get() == this) current.clear();
        worker.shutdownNow(); super.onDestroy();
    }
}
