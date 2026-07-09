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

    /// The chart's underlying zrender handle — used for raw canvas events
    /// (`dblclick`) that the chart instance doesn't re-emit for blank areas.
    pub type ZRender;

    #[wasm_bindgen(method, js_name = getZr)]
    pub fn get_zr(this: &EChart) -> ZRender;

    /// `zr.on(event, handler)` — subscribe to a raw canvas event.
    #[wasm_bindgen(method)]
    pub fn on(this: &ZRender, event: &str, handler: &js_sys::Function);

    /// `chart.getOption()` — the live, normalized option; read to learn the
    /// current dataZoom window.
    #[wasm_bindgen(method, js_name = getOption)]
    pub fn get_option(this: &EChart) -> JsValue;
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

/// Widen a dataZoom window `[start, end]` (percentages of the full range) by
/// 2× around its center, clamped to `[0, 100]` — one gradual "zoom out" step.
/// Repeated calls walk any window back to the full range; a degenerate
/// (zero-width) window opens to a 5% span so it can't get stuck.
pub(crate) fn widen_window(start: f64, end: f64) -> (f64, f64) {
    let span = ((end - start) * 2.0).clamp(5.0, 100.0);
    let center = (start + end) / 2.0;
    let mut s = center - span / 2.0;
    let mut e = center + span / 2.0;
    if s < 0.0 {
        e = (e - s).min(100.0);
        s = 0.0;
    }
    if e > 100.0 {
        s = (s - (e - 100.0)).max(0.0);
        e = 100.0;
    }
    (s, e)
}

/// The chart's current dataZoom window in percent, `(0, 100)` when the
/// option carries no dataZoom (never the case for our charts, but the
/// fallback keeps the handler total).
fn zoom_window(chart: &EChart) -> (f64, f64) {
    use wasm_bindgen::JsCast;
    let opt = chart.get_option();
    let first = js_sys::Reflect::get(&opt, &"dataZoom".into())
        .ok()
        .and_then(|v| v.dyn_into::<js_sys::Array>().ok())
        .and_then(|arr| (arr.length() > 0).then(|| arr.get(0)));
    let field = |obj: &JsValue, name: &str, default: f64| {
        js_sys::Reflect::get(obj, &name.into())
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(default)
    };
    match first {
        Some(dz) => (field(&dz, "start", 0.0), field(&dz, "end", 100.0)),
        None => (0.0, 100.0),
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
    // The dblclick closure must outlive the JS subscription; it's parked
    // here and dropped on cleanup (after dispose tears down zrender).
    let dblclick = StoredValue::new_local(None::<Closure<dyn FnMut()>>);
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
                    // Double-click steps the zoom back out: widen the current
                    // window 2× and dispatch it. In a connect group the action
                    // propagates, so all weather charts step out together.
                    let zoom_out = {
                        let instance = instance.clone();
                        Closure::wrap(Box::new(move || {
                            let (start, end) = zoom_window(&instance);
                            let (start, end) = widen_window(start, end);
                            let _ = instance.dispatch_action(&json(&format!(
                                r#"{{"type":"dataZoom","start":{start},"end":{end}}}"#
                            )));
                        }) as Box<dyn FnMut()>)
                    };
                    {
                        use wasm_bindgen::JsCast;
                        instance
                            .get_zr()
                            .on("dblclick", zoom_out.as_ref().unchecked_ref());
                    }
                    dblclick.set_value(Some(zoom_out));
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
    use super::widen_window;

    #[test]
    fn widens_twofold_around_center() {
        assert_eq!(widen_window(40.0, 60.0), (30.0, 70.0));
    }

    #[test]
    fn clamps_at_left_edge() {
        // [0, 10] doubles to a 20-wide window; can't go below 0, so it
        // grows rightward.
        assert_eq!(widen_window(0.0, 10.0), (0.0, 20.0));
    }

    #[test]
    fn clamps_at_right_edge() {
        assert_eq!(widen_window(90.0, 100.0), (80.0, 100.0));
    }

    #[test]
    fn full_range_is_a_fixed_point() {
        assert_eq!(widen_window(0.0, 100.0), (0.0, 100.0));
    }

    #[test]
    fn degenerate_window_gets_minimum_span() {
        // A zero-width window (fully zoomed) opens to a 5% span.
        assert_eq!(widen_window(50.0, 50.0), (47.5, 52.5));
    }

    #[test]
    fn near_full_span_caps_at_full_range() {
        // 2 × 60 caps at 100, centered on 50 → the full range.
        assert_eq!(widen_window(20.0, 80.0), (0.0, 100.0));
    }
}
