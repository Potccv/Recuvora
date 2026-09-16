//! Shared user-interface resources for browser and Tauri desktop delivery.
//!
//! The frontends render the same interface views and do not own business state,
//! approval policy, or execution permissions. Live data comes from the trusted
//! HTTP API; fixtures require explicit demo mode and cannot authorize actions.

#[cfg(test)]
#[path = "../../../tests/ui.rs"]
mod tests;

/// The sole embedded asset catalog; HTTP and Desktop consume the same public source.
#[cfg(all(feature = "web-ui", feature = "server"))]
pub(crate) fn asset(name: &str) -> Option<(&'static str, &'static [u8])> {
    let javascript = "text/javascript; charset=utf-8";
    Some(match name {
        "index.html" => (
            "text/html; charset=utf-8",
            include_bytes!("public/index.html"),
        ),
        "styles.css" => (
            "text/css; charset=utf-8",
            include_bytes!("public/styles.css"),
        ),
        "app.js" => (javascript, include_bytes!("public/app.js")),
        "api.js" => (javascript, include_bytes!("public/api.js")),
        "dom.js" => (javascript, include_bytes!("public/dom.js")),
        "history.js" => (javascript, include_bytes!("public/history.js")),
        "monitoring.js" => (javascript, include_bytes!("public/monitoring.js")),
        "plugin-monitoring.js" => (javascript, include_bytes!("public/plugin-monitoring.js")),
        "project-logs.js" => (javascript, include_bytes!("public/project-logs.js")),
        "refresh.js" => (javascript, include_bytes!("public/refresh.js")),
        "data.js" => (javascript, include_bytes!("public/data.js")),
        "shell.js" => (javascript, include_bytes!("public/shell.js")),
        "favicon.svg" => ("image/svg+xml", include_bytes!("public/favicon.svg")),
        "favicon.ico" => ("image/x-icon", include_bytes!("public/favicon.ico")),
        _ => return None,
    })
}
