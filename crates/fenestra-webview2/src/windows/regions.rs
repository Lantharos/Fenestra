// Drag / control region handling for the WebView2 backend.
//
// HWND-hosted WebView2 supports non-client hit-testing when
// `ICoreWebView2Settings9::IsNonClientRegionSupportEnabled` is true.
// Pages can then use `-webkit-app-region: drag` / `no-drag`. Builder-
// configured `drag_region` / `control_region` rectangles are injected as
// fixed overlays so apps that configure regions from Rust (the same API
// as the CEF OSR host) get identical frameless behavior without writing
// CSS themselves.

#![cfg(target_os = "windows")]

use fenestra_platform::WindowRegionRect;
use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2, ICoreWebView2Settings9};
use windows::core::Interface;

use crate::{
    WebView2Config, WebView2Result, WebView2WindowControlAction, WebView2WindowControlRegion,
    windows::bridge,
};

/// Enable WebView2 non-client region support so `-webkit-app-region`
/// and the injected drag overlays participate in caption hit-testing.
pub(crate) fn enable_non_client_region_support(webview: &ICoreWebView2) -> WebView2Result<()> {
    let settings = unsafe { webview.Settings() }.map_err(bridge::webview2_error)?;
    let Ok(settings9) = settings.cast::<ICoreWebView2Settings9>() else {
        return Ok(());
    };
    unsafe { settings9.SetIsNonClientRegionSupportEnabled(true) }
        .map_err(bridge::webview2_error)?;
    Ok(())
}

/// Inject fixed-position drag / exclusion / control overlays that map
/// the Rust builder regions onto WebView2's CSS app-region hit-testing.
pub(crate) fn install_region_script(
    webview: &ICoreWebView2,
    config: &WebView2Config,
) -> WebView2Result<()> {
    if !config.frameless
        && config.drag_regions.is_empty()
        && config.drag_exclusion_regions.is_empty()
        && config.control_regions.is_empty()
    {
        return Ok(());
    }
    let script = region_install_script(
        &config.drag_regions,
        &config.drag_exclusion_regions,
        &config.control_regions,
        config.frameless || !config.chrome.uses_native_decorations(),
    );
    let wide = bridge::wide_pwstr(&script);
    let completed = webview2_com::AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(
        Box::new(|_error, _id| Ok(())),
    );
    unsafe {
        webview
            .AddScriptToExecuteOnDocumentCreated(windows::core::PCWSTR(wide.as_ptr()), &completed)
    }
    .map_err(bridge::webview2_error)?;
    Ok(())
}

pub(crate) fn region_install_script(
    drag_regions: &[WindowRegionRect],
    exclusion_regions: &[WindowRegionRect],
    control_regions: &[WebView2WindowControlRegion],
    enable_resize_edges: bool,
) -> String {
    let drag_json = rects_to_json(drag_regions);
    let exclusion_json = rects_to_json(exclusion_regions);
    let control_json = control_regions_to_json(control_regions);
    let resize_json = if enable_resize_edges { "true" } else { "false" };
    let border = crate::windows::host_controls::FRAMELESS_RESIZE_BORDER;
    format!(
        r#"(function(){{
  if (window.__fenestraNcRegionsInstalled) return;
  window.__fenestraNcRegionsInstalled = true;
  var DRAG = {drag_json};
  var EXCLUSION = {exclusion_json};
  var CONTROLS = {control_json};
  var RESIZE = {resize_json};
  var root = null;
  function resolveRect(rect, winW) {{
    var x = rect.x < 0 ? winW + rect.x : rect.x;
    var w = rect.width >= 2147483647 ? Math.max(0, winW - x) : rect.width;
    return {{ x: x, y: rect.y, width: Math.max(0, w), height: Math.max(0, rect.height) }};
  }}
  function clear() {{
    if (root && root.parentNode) root.parentNode.removeChild(root);
    root = null;
  }}
  function postWindow(action) {{
    var url = 'fenestra://window/' + action;
    try {{
      if (window.chrome && window.chrome.webview && window.chrome.webview.postMessage) {{
        window.chrome.webview.postMessage(url);
        return;
      }}
    }} catch (e) {{}}
    location.href = url;
  }}
  function apply() {{
    clear();
    var winW = window.innerWidth || document.documentElement.clientWidth || 0;
    var winH = window.innerHeight || document.documentElement.clientHeight || 0;
    root = document.createElement('div');
    root.setAttribute('data-fenestra-nc-root', '1');
    root.style.cssText = 'position:fixed;inset:0;pointer-events:none;z-index:2147483000;';
    function addDrag(rect) {{
      var r = resolveRect(rect, winW);
      if (r.width <= 0 || r.height <= 0) return;
      var el = document.createElement('div');
      el.style.cssText = 'position:fixed;left:'+r.x+'px;top:'+r.y+'px;width:'+r.width+'px;height:'+r.height+'px;pointer-events:auto;-webkit-app-region:drag;app-region:drag;';
      root.appendChild(el);
    }}
    function addNoDrag(rect) {{
      var r = resolveRect(rect, winW);
      if (r.width <= 0 || r.height <= 0) return;
      var el = document.createElement('div');
      el.style.cssText = 'position:fixed;left:'+r.x+'px;top:'+r.y+'px;width:'+r.width+'px;height:'+r.height+'px;pointer-events:none;-webkit-app-region:no-drag;app-region:no-drag;';
      root.appendChild(el);
    }}
    function addControl(control) {{
      var r = resolveRect(control.rect, winW);
      if (r.width <= 0 || r.height <= 0) return;
      var el = document.createElement('button');
      el.type = 'button';
      el.setAttribute('aria-label', control.action);
      el.style.cssText = 'position:fixed;left:'+r.x+'px;top:'+r.y+'px;width:'+r.width+'px;height:'+r.height+'px;padding:0;margin:0;border:0;background:transparent;pointer-events:auto;-webkit-app-region:no-drag;app-region:no-drag;cursor:pointer;';
      function visual() {{
        return document.querySelector('.window-control.' + control.action);
      }}
      el.addEventListener('mouseenter', function() {{
        var btn = visual();
        if (btn) btn.classList.add('is-hover');
      }});
      el.addEventListener('mouseleave', function() {{
        var btn = visual();
        if (btn) btn.classList.remove('is-hover');
      }});
      el.addEventListener('click', function(ev) {{
        ev.preventDefault();
        ev.stopPropagation();
        var action = control.action;
        var btn = visual();
        if (btn) btn.classList.remove('is-hover');
        postWindow(action);
      }});
      root.appendChild(el);
    }}
    function addResize(edge) {{
      var el = document.createElement('div');
      var b = {border};
      var css = 'position:fixed;pointer-events:auto;-webkit-app-region:no-drag;app-region:no-drag;z-index:2147483001;background:transparent;';
      if (edge === 'left') css += 'left:0;top:'+b+'px;width:'+b+'px;height:'+Math.max(0,winH-2*b)+'px;cursor:ew-resize;';
      else if (edge === 'right') css += 'right:0;top:'+b+'px;width:'+b+'px;height:'+Math.max(0,winH-2*b)+'px;cursor:ew-resize;';
      else if (edge === 'top') css += 'left:'+b+'px;top:0;width:'+Math.max(0,winW-2*b)+'px;height:'+b+'px;cursor:ns-resize;';
      else if (edge === 'bottom') css += 'left:'+b+'px;bottom:0;width:'+Math.max(0,winW-2*b)+'px;height:'+b+'px;cursor:ns-resize;';
      else if (edge === 'top-left') css += 'left:0;top:0;width:'+b+'px;height:'+b+'px;cursor:nwse-resize;';
      else if (edge === 'top-right') css += 'right:0;top:0;width:'+b+'px;height:'+b+'px;cursor:nesw-resize;';
      else if (edge === 'bottom-left') css += 'left:0;bottom:0;width:'+b+'px;height:'+b+'px;cursor:nesw-resize;';
      else if (edge === 'bottom-right') css += 'right:0;bottom:0;width:'+b+'px;height:'+b+'px;cursor:nwse-resize;';
      el.style.cssText = css;
      el.addEventListener('mousedown', function(ev) {{
        if (ev.button !== 0) return;
        ev.preventDefault();
        ev.stopPropagation();
        postWindow('begin-resize/' + edge);
      }});
      root.appendChild(el);
    }}
    for (var i = 0; i < DRAG.length; i++) addDrag(DRAG[i]);
    for (var j = 0; j < EXCLUSION.length; j++) addNoDrag(EXCLUSION[j]);
    for (var k = 0; k < CONTROLS.length; k++) addControl(CONTROLS[k]);
    if (RESIZE) {{
      var edges = ['left','right','top','bottom','top-left','top-right','bottom-left','bottom-right'];
      for (var e = 0; e < edges.length; e++) addResize(edges[e]);
    }}
    document.documentElement.appendChild(root);
  }}
  function boot() {{
    apply();
    window.addEventListener('resize', apply);
  }}
  if (document.readyState === 'loading') {{
    document.addEventListener('DOMContentLoaded', boot, {{ once: true }});
  }} else {{
    boot();
  }}
}})();"#
    )
}

fn rects_to_json(rects: &[WindowRegionRect]) -> String {
    let values: Vec<serde_json::Value> = rects
        .iter()
        .map(|rect| {
            serde_json::json!({
                "x": rect.x,
                "y": rect.y,
                "width": rect.width,
                "height": rect.height,
            })
        })
        .collect();
    serde_json::to_string(&values).unwrap_or_else(|_| "[]".to_string())
}

fn control_regions_to_json(regions: &[WebView2WindowControlRegion]) -> String {
    let values: Vec<serde_json::Value> = regions
        .iter()
        .map(|region| {
            serde_json::json!({
                "action": region.action.as_str(),
                "rect": {
                    "x": region.rect.x,
                    "y": region.rect.y,
                    "width": region.rect.width,
                    "height": region.rect.height,
                }
            })
        })
        .collect();
    serde_json::to_string(&values).unwrap_or_else(|_| "[]".to_string())
}

#[allow(dead_code)]
pub(crate) fn apply_drag_regions(
    _hwnd: isize,
    _regions: &[WindowRegionRect],
) -> WebView2Result<()> {
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn control_action_label(action: WebView2WindowControlAction) -> &'static str {
    action.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WebView2WindowControlAction;
    use fenestra_platform::WindowRegionRect;

    #[test]
    fn region_script_includes_drag_and_controls() {
        let script = region_install_script(
            &[WindowRegionRect::new(0, 0, i32::MAX, 38)],
            &[],
            &[WebView2WindowControlRegion {
                action: WebView2WindowControlAction::Close,
                rect: WindowRegionRect::new(-36, 7, 24, 24),
            }],
            true,
        );
        assert!(script.contains("fenestra-nc"));
        assert!(script.contains("close"));
        assert!(script.contains("-webkit-app-region:drag"));
        assert!(script.contains("begin-resize"));
    }
}
