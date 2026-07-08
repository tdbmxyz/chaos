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
}

// wasm-bindgen doesn't derive `Clone` for extern types by itself; the
// instance is cached in a `StoredValue` (Home's chart Effect re-fetches it
// on every rerun rather than re-initializing), which needs it.
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
