//! Push notifications via [ntfy](https://ntfy.sh): service down/recovered
//! alerts from the monitor and calendar event reminders.
//!
//! Everything here is best-effort by design — a failed notification is a
//! warning in the log, never an error that reaches a caller. The feature
//! is fully off (no client, no task) when `[notifications].ntfy_url` is
//! unset.

use std::time::Duration;

use url::Url;

use crate::config::NotificationsConfig;

const TIMEOUT: Duration = Duration::from_secs(10);

// Wired up (constructed, stored on `AppState`, `send` called) in Task 3
// (service alerts) and Task 4 (calendar reminders) of the notifications
// plan; until then nothing outside tests reads these fields or calls
// `send`, so silence dead_code rather than leave clippy red.
#[allow(dead_code)]
pub struct Notifier {
    http: reqwest::Client,
    /// `{ntfy_url}/{topic}` — ntfy publishes with a plain POST to the topic.
    endpoint: Url,
    token: Option<String>,
}

impl Notifier {
    /// `None` when notifications aren't configured (`ntfy_url` unset).
    pub fn new(config: &NotificationsConfig) -> anyhow::Result<Option<Self>> {
        let Some(base) = config.ntfy_url.clone() else {
            return Ok(None);
        };
        let topic = config.topic.trim();
        anyhow::ensure!(
            !topic.is_empty(),
            "notifications.ntfy_url is set but notifications.topic is empty"
        );
        let endpoint = base
            .join(topic)
            .map_err(|e| anyhow::anyhow!("joining ntfy topic onto {base}: {e}"))?;
        Ok(Some(Self {
            http: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .user_agent(concat!("chaos/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("building ntfy http client"),
            endpoint,
            token: config.token.clone(),
        }))
    }

    /// Publish one notification. Best-effort: failures are logged and
    /// swallowed — a dead ntfy server must never take down chaos.
    #[allow(dead_code)]
    pub async fn send(&self, title: &str, message: &str, tags: &str, priority: &str) {
        let mut request = self
            .http
            .post(self.endpoint.clone())
            .header("Title", title)
            .header("Tags", tags)
            .header("Priority", priority)
            .body(message.to_string());
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        match request.send().await {
            Ok(resp) if !resp.status().is_success() => {
                tracing::warn!(status = %resp.status(), title, "ntfy rejected notification");
            }
            Ok(_) => tracing::debug!(title, "notification sent"),
            Err(err) => tracing::warn!(error = %err, title, "ntfy send failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::http::HeaderMap;
    use tokio::sync::Mutex;
    use url::Url;

    use super::*;
    use crate::config::NotificationsConfig;

    /// One request as seen by the stub ntfy server.
    #[derive(Debug, Clone)]
    struct Captured {
        path: String,
        title: Option<String>,
        priority: Option<String>,
        tags: Option<String>,
        authorization: Option<String>,
        body: String,
    }

    /// Stub ntfy: captures every POST and answers 200.
    async fn stub_ntfy() -> (Url, Arc<Mutex<Vec<Captured>>>) {
        let captured: Arc<Mutex<Vec<Captured>>> = Arc::default();
        let sink = captured.clone();
        let app = axum::Router::new().fallback(axum::routing::post(
            move |uri: axum::http::Uri, headers: HeaderMap, body: String| {
                let sink = sink.clone();
                let header = |name: &str| {
                    headers
                        .get(name)
                        .and_then(|v| v.to_str().ok())
                        .map(String::from)
                };
                let entry = Captured {
                    path: uri.path().to_string(),
                    title: header("title"),
                    priority: header("priority"),
                    tags: header("tags"),
                    authorization: header("authorization"),
                    body,
                };
                async move {
                    sink.lock().await.push(entry);
                    "ok"
                }
            },
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binding stub ntfy");
        let addr = listener.local_addr().expect("stub ntfy addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serving stub ntfy");
        });
        (
            format!("http://{addr}/").parse().expect("stub ntfy url"),
            captured,
        )
    }

    fn config(url: Url, token: Option<&str>) -> NotificationsConfig {
        NotificationsConfig {
            ntfy_url: Some(url),
            topic: "chaos".into(),
            token: token.map(String::from),
            ..NotificationsConfig::default()
        }
    }

    #[tokio::test]
    async fn send_posts_to_the_topic_with_headers_and_bearer() {
        let (url, captured) = stub_ntfy().await;
        let notifier = Notifier::new(&config(url, Some("tk_secret")))
            .expect("building notifier")
            .expect("notifier is configured");

        notifier
            .send(
                "jellyfin is down",
                "connection refused",
                "rotating_light",
                "high",
            )
            .await;

        let requests = captured.lock().await;
        assert_eq!(requests.len(), 1);
        let req = &requests[0];
        assert_eq!(req.path, "/chaos");
        assert_eq!(req.title.as_deref(), Some("jellyfin is down"));
        assert_eq!(req.tags.as_deref(), Some("rotating_light"));
        assert_eq!(req.priority.as_deref(), Some("high"));
        assert_eq!(req.authorization.as_deref(), Some("Bearer tk_secret"));
        assert_eq!(req.body, "connection refused");
    }

    #[tokio::test]
    async fn send_survives_an_unreachable_server() {
        // Nothing listens on this port; send must log and return, not panic.
        let notifier = Notifier::new(&config("http://127.0.0.1:1/".parse().expect("url"), None))
            .expect("building notifier")
            .expect("notifier is configured");
        notifier.send("t", "m", "calendar", "default").await;
    }

    #[test]
    fn unconfigured_and_misconfigured_notifier() {
        // No ntfy_url → feature off.
        assert!(
            Notifier::new(&NotificationsConfig::default())
                .expect("no url is fine")
                .is_none()
        );
        // URL without a topic is a startup error, not a silent no-op.
        let broken = NotificationsConfig {
            ntfy_url: Some("https://ntfy.sh".parse().expect("url")),
            ..NotificationsConfig::default()
        };
        assert!(Notifier::new(&broken).is_err());
    }
}
