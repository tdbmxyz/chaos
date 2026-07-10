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

    /// `chart.setOption(option, opts)` — option updates with update options
    /// (we pass `replaceMerge: ["series"]` so series dropped from the option
    /// are actually removed; plain merge mode keeps them forever).
    #[wasm_bindgen(method, js_name = setOption, catch)]
    pub fn set_option_with(this: &EChart, option: &JsValue, opts: &JsValue) -> Result<(), JsValue>;

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

    /// The chart's underlying zrender handle — used for raw canvas events
    /// (`dblclick`) that the chart instance doesn't re-emit for blank areas.
    pub type ZRender;

    #[wasm_bindgen(method, js_name = getZr)]
    pub fn get_zr(this: &EChart) -> ZRender;

    /// `zr.on(event, handler)` — subscribe to a raw canvas event.
    #[wasm_bindgen(method)]
    pub fn on(this: &ZRender, event: &str, handler: &js_sys::Function);
}

// wasm-bindgen doesn't derive `Clone` for extern types by itself; ChartCanvas
// caches the instance in a `StoredValue` for the component's lifetime, which
// needs it.
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

/// The shared "inside" dataZoom fragment used by every chart: wheel zooms
/// around the cursor, drag pans, touch pinches; wheel never pans
/// (moveOnMouseWheel) so page scroll stays predictable. No start/end —
/// ChartCanvas dispatches the default window once, so reactive re-renders
/// leave a user-adjusted window alone.
pub(crate) fn inside_zoom() -> serde_json::Value {
    serde_json::json!([{
        "type": "inside",
        "xAxisIndex": 0,
        "zoomOnMouseWheel": true,
        "moveOnMouseMove": true,
        "moveOnMouseWheel": false,
    }])
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

/// Dispatch a dataZoom action setting the window to `[start, end]` percent
/// of the full range.
fn zoom_to(chart: &EChart, (start, end): (f64, f64)) {
    let _ = chart.dispatch_action(&json(&format!(
        r#"{{"type":"dataZoom","start":{start},"end":{end}}}"#
    )));
}

/// A mounted ECharts instance: owns init, option updates, zoom gestures,
/// resize, and disposal. `option` is re-run reactively, so a builder that
/// reads signals re-renders the chart. When `group` is set, the chart joins
/// that ECharts connect-group (shared dataZoom + tooltip across every chart
/// in the group). `reset_zoom` is the default dataZoom window in percent —
/// applied once after the first render and again on every double-click
/// (options never carry `start`/`end`, so reactive re-renders leave a
/// user-adjusted window alone). `class` sizes the container.
#[component]
pub fn ChartCanvas(
    option: Callback<(), serde_json::Value>,
    // `into` lets callers pass `group="weather"` (wrapped to Some) or omit it (None).
    #[prop(optional, into)] group: Option<&'static str>,
    // `into`: pass a bare `(start, end)` tuple or omit for the full range.
    #[prop(optional, into)] reset_zoom: Option<(f64, f64)>,
    class: &'static str,
) -> impl IntoView {
    let reset_zoom = reset_zoom.unwrap_or((0.0, 100.0));
    let node = NodeRef::<leptos::html::Div>::new();
    let chart = StoredValue::new_local(None::<EChart>);
    // The dblclick closure must outlive the JS subscription; it's parked
    // here and dropped on cleanup (after dispose tears down zrender).
    let dblclick = StoredValue::new_local(None::<Closure<dyn FnMut()>>);
    // The default window is dispatched once, after the first set_option of
    // the mount (the dataZoom component must exist before the action).
    let zoomed = StoredValue::new_local(false);
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
                    // Double-click resets to the default window. In a
                    // connect group the action propagates, so all weather
                    // charts reset together.
                    let reset = {
                        let instance = instance.clone();
                        Closure::wrap(Box::new(move || {
                            zoom_to(&instance, reset_zoom);
                        }) as Box<dyn FnMut()>)
                    };
                    {
                        use wasm_bindgen::JsCast;
                        instance
                            .get_zr()
                            .on("dblclick", reset.as_ref().unchecked_ref());
                    }
                    dblclick.set_value(Some(reset));
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
        // replaceMerge on series only: retracted locations leave the chart
        // (plain merge would keep them in the tooltip forever), while every
        // other component — crucially the dataZoom — merges, preserving the
        // current zoom window as siblings stream in.
        let _ = instance.set_option_with(&opt, &json(r#"{"replaceMerge":["series"]}"#));
        // First render of this mount: apply the default window. Propagating
        // through the connect group is intended — a newly added location
        // realigns every synced chart, keeping the group consistent.
        if !zoomed.get_value() {
            zoomed.set_value(true);
            zoom_to(&instance, reset_zoom);
        }
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
        dblclick.set_value(None);
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

#[cfg(test)]
mod tests {
    #[test]
    fn inside_zoom_has_the_shared_gesture_flags() {
        let zoom = super::inside_zoom();
        assert_eq!(zoom[0]["type"], "inside");
        assert_eq!(zoom[0]["zoomOnMouseWheel"], true);
        assert_eq!(zoom[0]["moveOnMouseMove"], true);
        assert_eq!(zoom[0]["moveOnMouseWheel"], false);
        assert!(zoom[0]["start"].is_null());
    }
}
