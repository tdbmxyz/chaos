//! Minimal bindings to the vendored Apache ECharts bundle (loaded globally
//! from index.html). Only the surface the Home tab chart uses — options are
//! passed as JSON built with serde_json and parsed on the JS side.

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
