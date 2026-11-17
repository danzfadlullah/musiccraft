package io.github.bhimio1.musiccraft;

import android.app.NativeActivity;
import android.content.ClipData;
import android.content.Intent;
import android.content.pm.PackageInstaller;
import android.net.Uri;
import android.os.Build;
import android.util.Log;

/**
 * The one Java class in the app. NativeActivity never forwards
 * {@code onActivityResult} to native code, so this subclass stows the SAF
 * picker's answer in a static field that the Rust side polls through JNI
 * (crates/musiccraft-android-java, "saf.rs"). The URIs come back as one
 * newline-separated string; {@code !cancelled} and {@code !error:...} carry
 * the no-answer cases.
 *
 * <p>It is also the install session's status receiver: the in-app update
 * commits a {@link PackageInstaller} session with a PendingIntent for this
 * activity, and the session's answer arrives here as an intent. The one
 * answer that needs acting on is {@code STATUS_PENDING_USER_ACTION} — its
 * {@code EXTRA_INTENT} is the screen the user must see first (the install
 * confirmation, or the "install unknown apps" grant), and showing it is our
 * job. Without this, the session would wait forever and nothing would
 * appear, however correct the commit was.
 */
public class MusicCraftActivity extends NativeActivity {
    public static volatile String pickResult = null;
    /** The last install session status, for logcat; "status N" or "!start: ...". */
    public static volatile String installResult = null;

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode != 42) {
            return;
        }
        if (resultCode != RESULT_OK || data == null) {
            pickResult = "!cancelled";
            return;
        }
        StringBuilder uris = new StringBuilder();
        ClipData clip = data.getClipData();
        if (clip != null) {
            for (int i = 0; i < clip.getItemCount(); i++) {
                Uri uri = clip.getItemAt(i).getUri();
                if (uri != null) {
                    if (uris.length() > 0) {
                        uris.append('\n');
                    }
                    uris.append(uri.toString());
                }
            }
        }
        if (uris.length() == 0 && data.getData() != null) {
            uris.append(data.getDataString());
        }
        pickResult = uris.length() == 0 ? "!cancelled" : uris.toString();
    }

    @Override
    protected void onCreate(android.os.Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        handleInstallStatus(getIntent());
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        handleInstallStatus(intent);
    }

    /**
     * Acts on an install session's status intent: shows the screen the
     * system asks for when the user's action is pending. Anything else is
     * only recorded.
     */
    private void handleInstallStatus(Intent intent) {
        if (intent == null || !intent.hasExtra(PackageInstaller.EXTRA_STATUS)) {
            return;
        }
        int status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS,
                PackageInstaller.STATUS_FAILURE);
        installResult = "status " + status;
        Log.i("MusicCraft", "install session status " + status);
        if (status != PackageInstaller.STATUS_PENDING_USER_ACTION) {
            return;
        }
        Intent confirm;
        if (Build.VERSION.SDK_INT >= 33) {
            confirm = intent.getParcelableExtra(Intent.EXTRA_INTENT, Intent.class);
        } else {
            confirm = intent.getParcelableExtra(Intent.EXTRA_INTENT);
        }
        if (confirm == null) {
            installResult = "!no intent";
            return;
        }
        confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        try {
            startActivity(confirm);
        } catch (Exception e) {
            installResult = "!start: " + e;
            Log.w("MusicCraft", "install confirmation did not open", e);
        }
    }
}
