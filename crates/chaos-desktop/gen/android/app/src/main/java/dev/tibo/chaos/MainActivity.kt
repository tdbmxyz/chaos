package dev.tibo.chaos

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
  private var webView: WebView? = null
  private var pendingShare: String? = null

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
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
    // window.ChaosAndroid.openApp(package, fallbackUrl).
    webView.addJavascriptInterface(AppBridge(), "ChaosAndroid")
    pendingShare?.let { deliver(it) }
    pendingShare = null
  }

  inner class AppBridge {
    @JavascriptInterface
    fun openApp(pkg: String, url: String) {
      runOnUiThread {
        val launch = packageManager.getLaunchIntentForPackage(pkg)
        startActivity(launch ?: Intent(Intent.ACTION_VIEW, Uri.parse(url)))
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
