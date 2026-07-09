//! Minimal bindings to the vendored Apache ECharts bundle (loaded globally
//! from index.html). Provides reusable chart bindings plus a `ChartCanvas`
//! component used by both the Home and Weather tabs — options are passed as
//! JSON built with serde_json and parsed on the JS side.

use leptos::prelude::*;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    pub type EChart;

    /// `echarts.init(el)` — one chart instance bound to a DOM element.
    #[wasm_bindgen(js_namespace = echarts, catch)]
    pub fn init(el: &web_sys::HtmlElement) -> Result<EChart, JsValue>;

    #[wasm_bindgen(method, js_name = setOption, catch)]
    pub fn set_option(this: &EChart, option: &JsValue) -> Result<(), JsValue>;

    #[wasm_bindgen(method, js_name = dispatchAction, catch)]
    pub fn dispatch_action(this: &EChart, action: &JsValue) -> Result<(), JsValue>;

    #[wasm_bindgen(method, catch)]
    pub fn resize(this: &EChart) -> Result<(), JsValue>;

    #[wasm_bindgen(method, catch)]
    pub fn dispose(this: &EChart) -> Result<(), JsValue>;

    /// Assign the chart to a connect-group (charts sharing a group can be
    /// linked with `connect`).
    #[wasm_bindgen(method, setter = group)]
    pub fn set_group(this: &EChart, group: &str);

    /// `echarts.connect(group)` — link dataZoom + tooltip across every chart
    /// currently assigned to `group`.
    #[wasm_bindgen(js_namespace = echarts)]
    pub fn connect(group: &str);
}

// wasm-bindgen doesn't derive `Clone` for extern types by itself; the
// instance is cached in a `StoredValue` for the component's lifetime (the
// parent remounts TemperatureChart per data change, so the instance itself
// never survives a data change), which needs it.
impl Clone for EChart {
    fn clone(&self) -> Self {
        use wasm_bindgen::JsCast;
        JsValue::from(self).unchecked_into()
    }
}

/// Parse a JSON string into a JS object (NULL on bad input — callers treat
/// every interop step as fallible, the chart just stays empty).
pub fn json(raw: &str) -> JsValue {
    js_sys::JSON::parse(raw).unwrap_or(JsValue::NULL)
}

/// A CSS custom property from the active theme (empty string if unset). Reads
/// the DOM, so browser-only — calling it off-wasm panics (wasm-bindgen imports
/// can't run natively). Keep it out of anything unit-tested; inject colours via
/// `ChartColors` instead.
pub(crate) fn css_var(name: &str) -> String {
    web_sys::window()
        .and_then(|w| {
            let body = w.document()?.body()?;
            w.get_computed_style(&body).ok().flatten()
        })
        .and_then(|style| style.get_property_value(name).ok())
        .map(|value| value.trim().to_string())
        .unwrap_or_default()
}

/// Theme colours pulled from CSS variables, injected into option builders so
/// those builders stay pure (no DOM) and unit-testable off-wasm.
#[derive(Debug, Default, Clone)]
pub(crate) struct ChartColors {
    pub text: String,
    pub muted: String,
    pub border: String,
    pub surface: String,
    pub accent: String,
}

impl ChartColors {
    /// Read from the active theme (browser only — calls `css_var`).
    pub(crate) fn from_theme() -> Self {
        Self {
            text: css_var("--text"),
            muted: css_var("--muted"),
            border: css_var("--border"),
            surface: css_var("--surface"),
            accent: css_var("--accent"),
        }
    }
}

/// A mounted ECharts instance: owns init, option updates, drag-select zoom
/// arming, resize, and disposal. `option` is re-run reactively, so a builder
/// that reads signals re-renders the chart. When `group` is set, the chart
/// joins that ECharts connect-group (shared dataZoom + tooltip across every
/// chart in the group). `class` sizes the container (e.g. `"temp-chart"`).
#[component]
pub fn ChartCanvas(
    option: Callback<(), serde_json::Value>,
    // `into` lets callers pass `group="weather"` (wrapped to Some) or omit it (None).
    #[prop(optional, into)] group: Option<&'static str>,
    class: &'static str,
) -> impl IntoView {
    let node = NodeRef::<leptos::html::Div>::new();
    let chart = StoredValue::new_local(None::<EChart>);
    let failed = RwSignal::new(false);

    Effect::new(move |_| {
        let Some(el) = node.get() else {
            return;
        };
        let instance = match chart.get_value() {
            Some(instance) => instance,
            None => match init(&el) {
                Ok(instance) => {
                    if let Some(group) = group {
                        instance.set_group(group);
                    }
                    chart.set_value(Some(instance.clone()));
                    instance
                }
                // Bundle missing / init failed: show a message, page still works.
                Err(_) => {
                    failed.set(true);
                    return;
                }
            },
        };
        let opt = json(&option.run(()).to_string());
        let _ = instance.set_option(&opt);
        // Arm drag-select zoom (a toolbox feature, armed programmatically so no
        // toolbox icon must be clicked — the toolbox itself stays hidden).
        let _ = instance.dispatch_action(&json(
            r#"{"type":"takeGlobalCursor","key":"dataZoomSelect","dataZoomSelectActive":true}"#,
        ));
        // (Re)connect the group as members mount asynchronously.
        if let Some(group) = group {
            connect(group);
        }
    });

    let resize = window_event_listener(leptos::ev::resize, move |_| {
        if let Some(instance) = chart.get_value() {
            let _ = instance.resize();
        }
    });
    on_cleanup(move || {
        resize.remove();
        if let Some(instance) = chart.get_value() {
            let _ = instance.dispose();
        }
    });

    view! {
        <div class=class node_ref=node></div>
        {move || {
            failed
                .get()
                .then(|| view! { <p class="error">"Chart failed to load (echarts bundle missing?)"</p> })
        }}
    }
}
