package dev.spacemosaic;

import android.app.NativeActivity;
import android.app.AlertDialog;
import android.os.Bundle;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;
import android.view.Gravity;
import android.view.ViewGroup;
import android.widget.FrameLayout;
import java.lang.ref.WeakReference;
import java.util.Locale;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.os.CancellationSignal;
import android.provider.DocumentsContract;
import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicBoolean;

/** A narrow NativeActivity bridge. All provider queries run on one worker. */
public final class StorageActivity extends NativeActivity {
    static { System.loadLibrary("spacemosaic"); }
    private static final int PICK_TREE = 43;
    private final ExecutorService worker = Executors.newSingleThreadExecutor();
    private long pickerGeneration;
    private boolean pickerOpen;
    private CancellationSignal query;
    private final AtomicBoolean queryRunning = new AtomicBoolean(false);

    private static native boolean deliver(String json);
    private static native long begin();
    private static native boolean isCurrent(long generation);
    private static WeakReference<StorageActivity> current = new WeakReference<>(null);
    private JSONObject catalog;
    private JSONObject lastMessage;
    private LinearLayout browserRows;
    private Button launcher;
    private int appearance = 0;
    private int themeResource() {
        boolean dark = appearance == 2 || (appearance == 0 &&
            (getResources().getConfiguration().uiMode & android.content.res.Configuration.UI_MODE_NIGHT_MASK)
                == android.content.res.Configuration.UI_MODE_NIGHT_YES);
        return dark ? android.R.style.Theme_Material_NoActionBar : android.R.style.Theme_Material_Light_NoActionBar;
    }
    private android.content.Context viewContext() {
        return new android.view.ContextThemeWrapper(this, themeResource());
    }
    public void configureAppearance(int preference, String tag) {
        runOnUiThread(() -> {
            appearance = preference;
            try { catalog = readCatalog(tag); } catch (Exception ex) { /* Keep the current catalog. */ }
            if (launcher != null) launcher.setText(text("accessible_browser"));
        });
    }

    @Override public void onCreate(Bundle state) {
        setTheme(themeResource());
        super.onCreate(state);
        current = new WeakReference<>(this);
        String language = Locale.getDefault().getLanguage();
        String tag = language.equals("pt") ? "pt-BR" : language;
        try { catalog = readCatalog(tag); }
        catch (Exception ex) {
            try { catalog = readCatalog("en"); }
            catch (Exception unavailable) { catalog = new JSONObject(); }
        }
        Button button = new Button(viewContext());
        launcher = button;
        button.setText(text("accessible_browser"));
        button.setOnClickListener(view -> showBrowser());
        int height = (int)(56 * getResources().getDisplayMetrics().density);
        FrameLayout.LayoutParams params = new FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, height, Gravity.BOTTOM);
        button.setOnApplyWindowInsetsListener((view, insets) -> {
            view.setPadding(view.getPaddingLeft(), view.getPaddingTop(), view.getPaddingRight(),
                insets.getSystemWindowInsetBottom());
            return insets;
        });
        addContentView(button, params);
    }

    private JSONObject readCatalog(String tag) throws Exception {
        try (java.io.InputStream stream = getAssets().open(tag + ".json")) {
            java.io.ByteArrayOutputStream output = new java.io.ByteArrayOutputStream();
            byte[] buffer = new byte[4096]; int count;
            while ((count = stream.read(buffer)) != -1) output.write(buffer, 0, count);
            return new JSONObject(new String(output.toByteArray(), java.nio.charset.StandardCharsets.UTF_8));
        }
    }
    private String text(String key) { return catalog.optString(key, key); }
    private void showBrowser() {
        LinearLayout content = new LinearLayout(viewContext()); content.setOrientation(LinearLayout.VERTICAL);
        Button choose = new Button(viewContext()); choose.setText(text("choose"));
        choose.setOnClickListener(view -> pickTree(begin())); content.addView(choose);
        ScrollView scroll = new ScrollView(viewContext()); browserRows = new LinearLayout(viewContext());
        browserRows.setOrientation(LinearLayout.VERTICAL); scroll.addView(browserRows); content.addView(scroll);
        updateBrowser();
        AlertDialog dialog = new AlertDialog.Builder(viewContext()).setTitle(text("accessible_browser"))
            .setView(content).setNegativeButton(text("close"), (d, which) -> {}).create();
        dialog.setOnDismissListener(d -> browserRows = null); dialog.show();
    }
    private void updateBrowser() {
        if (browserRows == null) return;
        browserRows.removeAllViews();
        addText(text("mobile_scope"));
        if (lastMessage == null) return;
        addText(text(lastMessage.optString("status", "unavailable")));
        if (lastMessage.optBoolean("truncated")) addText(text("truncated"));
        JSONArray entries = lastMessage.optJSONArray("entries");
        if (entries == null) return;
        for (int index = 0; index < entries.length(); index++) {
            JSONObject entry = entries.optJSONObject(index); if (entry == null) continue;
            String size = entry.isNull("logical_bytes") ? text("unknown")
                : java.text.NumberFormat.getIntegerInstance().format(entry.optLong("logical_bytes"));
            addText(entry.optString("name") + "\n" + text("logical_bytes") + ": " + size
                + "\n" + entry.optString("locator"));
        }
    }
    private void addText(String content) {
        TextView label = new TextView(viewContext()); label.setText(content); label.setTextSize(16);
        int padding = (int)(12 * getResources().getDisplayMetrics().density);
        label.setPadding(padding, padding, padding, padding); browserRows.addView(label);
    }

    public void pickTree(long generation) {
        runOnUiThread(() -> {
            if (queryRunning.get()) {
                if (query != null) query.cancel();
                send(generation, "unavailable", null, new JSONArray(), false, false, "provider_busy");
                return;
            }
            if (pickerOpen) {
                send(generation, "unavailable", null, new JSONArray(), false, false, "picker_busy");
                return;
            }
            if (query != null) query.cancel();
            lastMessage = null; updateBrowser();
            pickerGeneration = generation;
            Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE);
            intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION
                    | Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION);
            try {
                pickerOpen = true;
                startActivityForResult(intent, PICK_TREE);
            } catch (RuntimeException ex) {
                pickerOpen = false;
                send(generation, "unavailable", null, new JSONArray(), false, false, "picker_unavailable");
            }
        });
    }

    @Override protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        if (request != PICK_TREE) return;
        pickerOpen = false;
        final long generation = pickerGeneration;
        if (result != RESULT_OK || data == null || data.getData() == null) {
            send(generation, "cancelled", null, new JSONArray(), false, false, null);
            return;
        }
        final Uri tree = data.getData();
        boolean retained = false;
        if ((data.getFlags() & Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION) != 0) {
            try {
                int read = data.getFlags() & Intent.FLAG_GRANT_READ_URI_PERMISSION;
                if (read != 0) {
                    getContentResolver().takePersistableUriPermission(tree, read);
                    retained = true;
                }
            } catch (SecurityException ex) {
                // A temporary read grant may still be usable. Do not claim persistence.
            }
        }
        final boolean persisted = retained;
        final CancellationSignal cancellation = new CancellationSignal();
        query = cancellation;
        queryRunning.set(true);
        worker.execute(() -> {
            try { enumerate(generation, tree, persisted, cancellation); }
            finally { queryRunning.set(false); }
        });
    }

    private void enumerate(long generation, Uri tree, boolean persisted, CancellationSignal cancellation) {
        JSONArray entries = new JSONArray();
        boolean truncated = false;
        String status = "ready";
        String error = null;
        String[] projection = {
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE,
            DocumentsContract.Document.COLUMN_SIZE
        };
        try {
            Uri children = DocumentsContract.buildChildDocumentsUriUsingTree(
                    tree, DocumentsContract.getTreeDocumentId(tree));
            try (Cursor cursor = getContentResolver().query(children, projection, null, null, null, cancellation)) {
                if (cursor == null) throw new IllegalStateException("null_cursor");
                while (cursor.moveToNext()) {
                    cancellation.throwIfCanceled();
                    if (entries.length() == 32) { truncated = true; break; }
                    String id = cursor.getString(0);
                    String mime = cursor.getString(2);
                    boolean directory = DocumentsContract.Document.MIME_TYPE_DIR.equals(mime);
                    JSONObject entry = new JSONObject();
                    entry.put("name", cursor.isNull(1) ? id : cursor.getString(1));
                    entry.put("locator", DocumentsContract.buildDocumentUriUsingTree(tree, id).toString());
                    entry.put("directory", directory);
                    entry.put("logical_bytes", directory || cursor.isNull(3) || cursor.getLong(3) < 0
                            ? JSONObject.NULL : cursor.getLong(3));
                    entry.put("allocated_bytes", JSONObject.NULL);
                    entries.put(entry);
                }
            }
        } catch (SecurityException ex) {
            status = "denied"; error = "access_revoked";
        } catch (Exception ex) {
            status = "unavailable"; error = "provider_unavailable";
        }
        send(generation, status, tree.toString(), entries, truncated, persisted, error);
    }

    private static void send(long generation, String status, String scope, JSONArray entries,
                             boolean truncated, boolean persisted, String error) {
        try {
            JSONObject result = new JSONObject();
            result.put("generation", generation);
            result.put("status", status);
            result.put("scope", scope == null ? JSONObject.NULL : scope);
            result.put("entries", entries);
            result.put("truncated", truncated);
            result.put("persisted", persisted);
            result.put("error", error == null ? JSONObject.NULL : error);
            String encoded = result.toString();
            if (encoded.getBytes(java.nio.charset.StandardCharsets.UTF_8).length > 256 * 1024) {
                send(generation, "unavailable", null, new JSONArray(), false, false, "message_too_large");
                return;
            }
            if (deliver(encoded)) {
                StorageActivity activity = current.get();
                if (activity != null) activity.runOnUiThread(() -> {
                    if (isCurrent(generation)) { activity.lastMessage = result; activity.updateBrowser(); }
                });
            }
        } catch (JSONException ex) {
            throw new IllegalStateException("bridge_encoding", ex);
        }
    }

    @Override protected void onDestroy() {
        if (query != null) query.cancel();
        current.clear();
        worker.shutdownNow();
        super.onDestroy();
    }
}
