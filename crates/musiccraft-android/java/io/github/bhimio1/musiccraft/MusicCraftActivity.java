package io.github.bhimio1.musiccraft;

import android.app.NativeActivity;
import android.content.ClipData;
import android.content.Intent;
import android.net.Uri;

/**
 * The one Java class in the app: NativeActivity never forwards
 * {@code onActivityResult} to native code, so this subclass stows the SAF
 * picker's answer in a static field that the Rust side polls through JNI
 * (crates/musiccraft-android-java, "saf.rs"). The URIs come back as one
 * newline-separated string; {@code !cancelled} and {@code !error:...} carry
 * the no-answer cases.
 */
public class MusicCraftActivity extends NativeActivity {
    public static volatile String pickResult = null;

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
}
