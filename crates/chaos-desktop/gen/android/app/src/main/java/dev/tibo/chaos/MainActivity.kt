package dev.tibo.chaos

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.view.View
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  private var webView: WebView? = null
  private var pendingShare: String? = null

  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    // Android 15+ enforces edge-to-edge for apps targeting SDK 35+ (and the
    // opt-out is gone at 36), so the webview draws under the status and
    // gesture bars. Pad the content view by the system-bar (and cutout)
    // insets instead, keeping the top bar and bottom menu reachable.
    val content = findViewById<View>(android.R.id.content)
    ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
      val bars = insets.getInsets(
        WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout()
      )
      view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
      WindowInsetsCompat.CONSUMED
    }
    pendingShare = sharedText(intent)
  }

  // Share sheet target: hand the shared text to the web app, which routes
  // it into the links quick-add (see ShareRedirect in chaos-ui).
  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    sharedText(intent)?.let { deliver(it) }
  }

  override fun onWebViewCreate(webView: WebView) {
    this.webView = webView
    // Companion apps ("plugins" like yomu): the web UI calls
    // window.ChaosAndroid.openApp(package); when that returns false, the
    // web UI lets the bookmark URL open normally (VIEW intent / browser).
    webView.addJavascriptInterface(AppBridge(), "ChaosAndroid")
    pendingShare?.let { deliver(it) }
    pendingShare = null
  }

  inner class AppBridge {
    // True when the native app claimed the tap; false (not installed) tells
    // the web UI to let the bookmark URL open normally (VIEW intent / browser).
    @JavascriptInterface
    fun openApp(pkg: String): Boolean {
      val launch = packageManager.getLaunchIntentForPackage(pkg) ?: return false
      runOnUiThread { startActivity(launch) }
      return true
    }

    // Outbound links leave the webview through a VIEW intent, so an
    // installed app that registered the URL (Jellyfin, Immich…) claims
    // it, and the default browser gets it otherwise.
    @JavascriptInterface
    fun openUrl(url: String) {
      runOnUiThread {
        try {
          startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)))
        } catch (_: Exception) {
          // no handler at all: swallow, the link simply doesn't open
        }
      }
    }
  }

  private fun sharedText(intent: Intent?): String? =
    if (intent?.action == Intent.ACTION_SEND) intent.getStringExtra(Intent.EXTRA_TEXT) else null

  private fun deliver(text: String) {
    val view = webView ?: return
    // Only the root path is guaranteed to resolve in the asset protocol,
    // so the share rides in as a query parameter.
    view.post { view.loadUrl("http://tauri.localhost/?share=" + Uri.encode(text)) }
  }
}
