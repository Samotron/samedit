//! HTTP request/response view-model (v0.11 M11.4).
//!
//! Mirrors the notebook layering ([`cockpit_notebook::Notebook`] →
//! [`Cell`](cockpit_notebook::Cell)): a [`HttpView`] holds the parsed
//! [`Collection`] plus the per-request run state — last response, last
//! error, send status, selected tab. The view is a pure function of
//! state: the binary calls into it on every frame and the painter reads
//! [`response_view()`](HttpView::response_view) without any
//! GPU/network/clock dependencies of its own (AGENTS §2 #2).
//!
//! Two extra pieces beyond the obvious mirror:
//!
//! - [`SplitLayout`] computes the top-half-request / bottom-half-response
//!   split inside the editor pane. Same plumbing as the M4.7 mouse-drag;
//!   the painter calls `compute_split()` once per frame.
//! - [`ResponseView`] is the headless equivalent of "what the
//!   right-hand tab actually shows" — pretty-printed body, header table,
//!   timing summary, or curl-`-v`-style raw frame. Switching tabs is a
//!   palette command (M11.5); this crate just exposes the view-model
//!   surface.

use std::time::Duration;

use cockpit_http::{
    Collection, Environment, HttpError, HttpMethod, PreparedRequest, Request, Response,
};

use crate::Rect;

/// The four tabs in the response panel. Matches `<leader>h1..h4` in M11.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResponseTab {
    /// Pretty-printed body — JSON / XML / plain — driven by `Content-Type`.
    #[default]
    Body,
    /// Header table.
    Headers,
    /// Elapsed time, redirect hops, final URL.
    Timing,
    /// curl-`-v`-style status line + headers + raw body bytes.
    Raw,
}

impl ResponseTab {
    /// All tabs in display order. Used by the painter to draw the tab strip.
    pub fn all() -> [ResponseTab; 4] {
        [Self::Body, Self::Headers, Self::Timing, Self::Raw]
    }

    /// Display label for the tab strip.
    pub fn label(self) -> &'static str {
        match self {
            Self::Body => "Body",
            Self::Headers => "Headers",
            Self::Timing => "Timing",
            Self::Raw => "Raw",
        }
    }

    /// Move to the next tab (wraps).
    pub fn next(self) -> Self {
        match self {
            Self::Body => Self::Headers,
            Self::Headers => Self::Timing,
            Self::Timing => Self::Raw,
            Self::Raw => Self::Body,
        }
    }

    /// Move to the previous tab (wraps).
    pub fn prev(self) -> Self {
        match self {
            Self::Body => Self::Raw,
            Self::Headers => Self::Body,
            Self::Timing => Self::Headers,
            Self::Raw => Self::Timing,
        }
    }
}

/// Status of one request in the view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SendStatus {
    /// Never sent (or the response has been cleared).
    #[default]
    Idle,
    /// Engine call is in flight.
    Sending,
    /// Last call succeeded (any HTTP status — even 4xx/5xx — counts as
    /// "the engine round-tripped"). Distinct from `Failed`, which means
    /// the engine itself errored.
    Ok,
    /// Last call returned an [`HttpError`].
    Failed,
    /// User tripped the cancel handle before the engine returned.
    Cancelled,
}

/// Per-request state in [`HttpView`]: latest response, latest error,
/// current send status.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestRun {
    pub status: SendStatus,
    pub response: Option<Response>,
    pub error: Option<HttpError>,
}

impl RequestRun {
    /// True when there's a real response to render (vs the empty initial
    /// state or a pure engine error).
    pub fn has_response(&self) -> bool {
        self.response.is_some()
    }
}

/// Headless view-model for a Bruno collection inside the editor pane.
#[derive(Debug, Clone, PartialEq)]
pub struct HttpView {
    collection: Collection,
    /// Index of the selected request; saturates as the collection
    /// changes. `None` only when the collection has no requests.
    selected: Option<usize>,
    /// Active environment name. `None` ⇒ run with an empty env (templates
    /// without `{{vars}}` still work).
    active_env: Option<String>,
    /// Per-request run state, indexed in sync with `collection.requests`.
    runs: Vec<RequestRun>,
    /// Currently selected response tab.
    tab: ResponseTab,
    /// Fraction of the editor pane height reserved for the request half.
    /// Defaults to 0.5; clamped to a small viable band so the user can't
    /// drag a panel to zero pixels.
    split_ratio: f32,
}

impl HttpView {
    /// New view over `collection`. Auto-selects the first request if any
    /// and the first environment alphabetically (Bruno's `environments/`
    /// listing is already stable-sorted at load time).
    pub fn new(collection: Collection) -> Self {
        let selected = (!collection.requests.is_empty()).then_some(0);
        let runs = vec![RequestRun::default(); collection.requests.len()];
        let active_env = collection.environments.first().map(|env| env.name.clone());
        Self {
            collection,
            selected,
            active_env,
            runs,
            tab: ResponseTab::default(),
            split_ratio: 0.5,
        }
    }

    /// Borrow the collection. Useful for tests + the M11.5 palette
    /// that lists environments.
    pub fn collection(&self) -> &Collection {
        &self.collection
    }

    /// Currently selected request, if any.
    pub fn selected_request(&self) -> Option<&Request> {
        self.selected
            .and_then(|index| self.collection.requests.get(index))
    }

    /// Index of the selected request, if any.
    pub fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    /// Select a specific request by index. Out-of-range indices clamp to
    /// the last request rather than silently no-op-ing.
    pub fn select(&mut self, index: usize) {
        if self.collection.requests.is_empty() {
            self.selected = None;
            return;
        }
        let clamped = index.min(self.collection.requests.len() - 1);
        self.selected = Some(clamped);
    }

    /// Move the selection forward (saturates at the last request).
    pub fn select_next(&mut self) {
        let Some(current) = self.selected else {
            self.selected = (!self.collection.requests.is_empty()).then_some(0);
            return;
        };
        if current + 1 < self.collection.requests.len() {
            self.selected = Some(current + 1);
        }
    }

    /// Move the selection backward (saturates at 0).
    pub fn select_prev(&mut self) {
        if let Some(current) = self.selected {
            self.selected = Some(current.saturating_sub(1));
        }
    }

    /// Switch the active environment. Pass `None` for "no environment".
    /// Returns `Err` if the named environment doesn't exist — the caller
    /// surfaces a status warning rather than silently dropping the input.
    pub fn switch_environment(&mut self, name: Option<&str>) -> Result<(), UnknownEnvironment> {
        match name {
            None => {
                self.active_env = None;
                Ok(())
            }
            Some(name) => {
                if !self
                    .collection
                    .environments
                    .iter()
                    .any(|env| env.name == name)
                {
                    return Err(UnknownEnvironment {
                        name: name.to_string(),
                    });
                }
                self.active_env = Some(name.to_string());
                Ok(())
            }
        }
    }

    /// Currently active environment, looked up by name. Returns `None` if
    /// either the user picked "no environment" or the name was cleared
    /// out by a reload.
    pub fn active_environment(&self) -> Option<&Environment> {
        let name = self.active_env.as_deref()?;
        self.collection
            .environments
            .iter()
            .find(|env| env.name == name)
    }

    /// Name of the active environment (`None` ⇒ none selected).
    pub fn active_environment_name(&self) -> Option<&str> {
        self.active_env.as_deref()
    }

    /// Per-request run state for the selected request.
    pub fn selected_run(&self) -> Option<&RequestRun> {
        self.selected.and_then(|index| self.runs.get(index))
    }

    /// Mutable run for the selected request — used by the M11.5 send
    /// command to update status mid-flight.
    fn selected_run_mut(&mut self) -> Option<&mut RequestRun> {
        let index = self.selected?;
        self.runs.get_mut(index)
    }

    /// Move the selected request to "sending". Idempotent if there's no
    /// selection (the M11.5 command would never reach here in that case).
    pub fn mark_sending(&mut self) {
        if let Some(run) = self.selected_run_mut() {
            run.status = SendStatus::Sending;
            run.response = None;
            run.error = None;
        }
    }

    /// Record the result of an engine call. `Ok(response)` lands as
    /// `SendStatus::Ok`; an engine [`HttpError::Cancelled`] becomes
    /// `SendStatus::Cancelled`; anything else becomes `SendStatus::Failed`.
    pub fn apply_result(&mut self, result: Result<Response, HttpError>) {
        let Some(run) = self.selected_run_mut() else {
            return;
        };
        match result {
            Ok(response) => {
                run.status = SendStatus::Ok;
                run.response = Some(response);
                run.error = None;
            }
            Err(HttpError::Cancelled) => {
                run.status = SendStatus::Cancelled;
                run.response = None;
                run.error = Some(HttpError::Cancelled);
            }
            Err(err) => {
                run.status = SendStatus::Failed;
                run.response = None;
                run.error = Some(err);
            }
        }
    }

    /// Currently selected response tab.
    pub fn response_tab(&self) -> ResponseTab {
        self.tab
    }

    /// Switch directly to a specific tab. Tab keys (M11.5 `<leader>h1..h4`)
    /// land here.
    pub fn set_response_tab(&mut self, tab: ResponseTab) {
        self.tab = tab;
    }

    /// Cycle to the next tab.
    pub fn next_tab(&mut self) {
        self.tab = self.tab.next();
    }

    /// Cycle to the previous tab.
    pub fn prev_tab(&mut self) {
        self.tab = self.tab.prev();
    }

    /// Current split ratio (0.0 ⇒ all response, 1.0 ⇒ all request).
    pub fn split_ratio(&self) -> f32 {
        self.split_ratio
    }

    /// Set the request/response split ratio. Values are clamped to
    /// `[0.15, 0.85]` so the user can't drag a half to zero (matches the
    /// notebook editor split's clamp).
    pub fn set_split_ratio(&mut self, ratio: f32) {
        self.split_ratio = ratio.clamp(0.15, 0.85);
    }

    /// Compute the request/response split inside `viewport`. The painter
    /// calls this once per frame.
    pub fn compute_split(&self, viewport: Rect) -> SplitLayout {
        let request_height = ((viewport.height as f32) * self.split_ratio)
            .round()
            .max(1.0) as u32;
        let request_height = request_height.min(viewport.height.saturating_sub(1));
        let response_height = viewport.height.saturating_sub(request_height);
        SplitLayout {
            request: Rect::new(viewport.x, viewport.y, viewport.width, request_height),
            response: Rect::new(
                viewport.x,
                viewport.y + request_height,
                viewport.width,
                response_height,
            ),
        }
    }

    /// Render the active tab's content for the selected request. Returns
    /// `None` if either nothing is selected or no response has landed yet.
    pub fn response_view(&self) -> Option<ResponseView<'_>> {
        let run = self.selected_run()?;
        let response = run.response.as_ref()?;
        Some(ResponseView::for_tab(response, self.tab))
    }
}

/// Top-half / bottom-half rectangles for the editor pane when an
/// `.bru` request is open. Computed by [`HttpView::compute_split`];
/// consumed by the binary's painter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitLayout {
    /// Top half — the request editor (Vim mode applies normally).
    pub request: Rect,
    /// Bottom half — the response panel (tab strip + active tab body).
    pub response: Rect,
}

/// Headless render of the selected tab's content. Plain data so the
/// painter has no logic to do beyond layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseView<'a> {
    /// Pretty-printed body, ready to flow into the text view.
    Body { content_type: String, text: String },
    /// Header rows in declaration order.
    Headers(Vec<(&'a str, &'a str)>),
    /// Timing summary — elapsed wall-clock, redirect count, final URL.
    Timing {
        elapsed: Duration,
        redirects: usize,
        final_url: &'a str,
    },
    /// curl-`-v`-style frame for debugging.
    Raw(String),
}

impl<'a> ResponseView<'a> {
    fn for_tab(response: &'a Response, tab: ResponseTab) -> Self {
        match tab {
            ResponseTab::Body => {
                let content_type = response
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                    .map(|(_, v)| v.as_str())
                    .unwrap_or("application/octet-stream")
                    .to_string();
                let text = pretty_print_body(&response.body, &content_type);
                Self::Body { content_type, text }
            }
            ResponseTab::Headers => Self::Headers(
                response
                    .headers
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect(),
            ),
            ResponseTab::Timing => Self::Timing {
                elapsed: response.elapsed,
                redirects: response.redirects.len(),
                final_url: &response.final_url,
            },
            ResponseTab::Raw => Self::Raw(render_raw(response)),
        }
    }
}

/// Render a response in curl-`-v` style: status line + headers + blank
/// line + body. Used by [`ResponseTab::Raw`].
fn render_raw(response: &Response) -> String {
    let mut out = String::with_capacity(128 + response.body.len());
    out.push_str(&format!("HTTP/1.1 {}\n", response.status));
    for (name, value) in &response.headers {
        out.push_str(&format!("{name}: {value}\n"));
    }
    out.push('\n');
    out.push_str(&String::from_utf8_lossy(&response.body));
    out
}

/// Pretty-print a body if we recognise the content type. JSON gets
/// re-indented with two spaces; other types fall back to a UTF-8-lossy
/// string. We intentionally never call `serde_json` here — the parsed
/// document would lose original key order, and large bodies would
/// double-allocate. A hand-rolled brace-walker is the right tool.
fn pretty_print_body(body: &[u8], content_type: &str) -> String {
    let lossy = String::from_utf8_lossy(body);
    if content_type
        .split(';')
        .next()
        .map(str::trim)
        .map(|primary| primary.eq_ignore_ascii_case("application/json"))
        .unwrap_or(false)
    {
        return pretty_print_json(&lossy);
    }
    lossy.into_owned()
}

/// Re-indent a JSON document so reviewers can scan it. Skips inside
/// string literals (so `"a, b"` inside JSON doesn't get a newline added
/// after the comma). If the input isn't well-formed the output falls
/// back to the original text — the engine still ran, we just can't
/// pretty-print invalid JSON.
fn pretty_print_json(source: &str) -> String {
    let mut out = String::with_capacity(source.len() + 16);
    let mut indent: usize = 0;
    let mut in_string = false;
    let mut escape = false;
    let bytes = source.as_bytes();
    for &byte in bytes {
        let ch = byte as char;
        if in_string {
            out.push(ch);
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => {
                in_string = true;
                out.push(ch);
            }
            '{' | '[' => {
                out.push(ch);
                indent += 1;
                out.push('\n');
                push_indent(&mut out, indent);
            }
            '}' | ']' => {
                indent = indent.saturating_sub(1);
                out.push('\n');
                push_indent(&mut out, indent);
                out.push(ch);
            }
            ',' => {
                out.push(ch);
                out.push('\n');
                push_indent(&mut out, indent);
            }
            ':' => {
                out.push(ch);
                out.push(' ');
            }
            ch if ch.is_ascii_whitespace() => {
                // Drop incoming whitespace; we control all spacing.
            }
            _ => out.push(ch),
        }
    }
    out
}

fn push_indent(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push_str("  ");
    }
}

/// [`HttpView::switch_environment`] error: the named environment isn't
/// in the loaded collection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownEnvironment {
    pub name: String,
}

impl std::fmt::Display for UnknownEnvironment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no environment named `{}` in this collection", self.name)
    }
}

impl std::error::Error for UnknownEnvironment {}

/// Combine [`HttpView`] with an [`HttpEngine`](cockpit_http::HttpEngine)
/// to actually fire the selected request. Kept as a free function so
/// `HttpView` itself stays pure data + state-machine — the binary owns
/// the engine lifetime, the view owns the state.
///
/// Returns the [`HttpMethod`] + URL that was sent (so callers can log
/// it) and updates the view in-place with the result.
pub fn send_selected<E>(
    view: &mut HttpView,
    engine: &E,
    cancel: &cockpit_http::CancelHandle,
) -> Result<SentSummary, SendError>
where
    E: cockpit_http::HttpEngine,
{
    let Some(request) = view.selected_request().cloned() else {
        return Err(SendError::NoSelection);
    };
    let env = view
        .active_environment()
        .cloned()
        .unwrap_or_else(|| Environment {
            name: String::new(),
            vars: Default::default(),
        });
    let prepared = cockpit_http::prepare_request(&request, &env).map_err(SendError::Prepare)?;
    let summary = SentSummary {
        method: prepared.method,
        url: prepared.url.clone(),
    };
    view.mark_sending();
    let result = engine.send(prepared, cancel);
    view.apply_result(result);
    Ok(summary)
}

/// Successful kick-off of [`send_selected`] — captures the prepared
/// method/URL so the caller can log it before the engine returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentSummary {
    pub method: HttpMethod,
    pub url: String,
}

/// Failure modes for [`send_selected`]. The engine's own
/// [`HttpError`] lands inside the view (`apply_result`), not here.
#[derive(Debug)]
pub enum SendError {
    /// The collection has no requests, or none is selected.
    NoSelection,
    /// `prepare_request` failed before we ever called the engine.
    Prepare(cockpit_http::PrepareError),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSelection => write!(f, "no request selected"),
            Self::Prepare(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for SendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NoSelection => None,
            Self::Prepare(err) => Some(err),
        }
    }
}

/// Borrow the [`PreparedRequest`] for the currently selected request
/// without firing the engine. Useful for the `Http: Copy As cURL`
/// command (M11.5) and for tests asserting on interpolation.
pub fn prepare_selected(view: &HttpView) -> Result<PreparedRequest, SendError> {
    let request = view.selected_request().ok_or(SendError::NoSelection)?;
    let env = view
        .active_environment()
        .cloned()
        .unwrap_or_else(|| Environment {
            name: String::new(),
            vars: Default::default(),
        });
    cockpit_http::prepare_request(request, &env).map_err(SendError::Prepare)
}

/// Render the currently selected request as a `curl` invocation,
/// against the active environment. Powers the M11.5 `Http: Copy As
/// cURL` command — the binary takes this string and pushes it to the
/// clipboard.
pub fn copy_selected_as_curl(view: &HttpView) -> Result<String, SendError> {
    let prepared = prepare_selected(view)?;
    Ok(prepared.to_curl())
}

/// Stable command ids for the M11.5 HTTP commands. Kept in one place
/// so the binary's dispatch table, the palette title source, and
/// future Lua extension bindings all agree on the spellings.
///
/// IDs use the `http.` prefix to keep them clustered alongside the
/// `pane.` / `editor.` / `file.` namespaces in `cockpit-ui::command_ids`.
pub mod command_ids {
    /// `Http: Send Request` — default `<leader>hs`.
    pub const SEND_REQUEST: &str = "http.send_request";
    /// `Http: Send All In Folder`.
    pub const SEND_ALL_IN_FOLDER: &str = "http.send_all_in_folder";
    /// `Http: Switch Environment` — default `<leader>he`.
    pub const SWITCH_ENVIRONMENT: &str = "http.switch_environment";
    /// `Http: Copy As cURL`.
    pub const COPY_AS_CURL: &str = "http.copy_as_curl";
    /// `Http: Save Response To File`.
    pub const SAVE_RESPONSE: &str = "http.save_response";
    /// `Http: Next Response Tab` — drives the cycling form of the tab
    /// switcher; per-tab direct selects (`http.tab.body` etc.) below.
    pub const NEXT_TAB: &str = "http.next_tab";
    /// `Http: Previous Response Tab`.
    pub const PREV_TAB: &str = "http.prev_tab";
    /// `Http: Show Body Tab` — default `<leader>h1`.
    pub const TAB_BODY: &str = "http.tab.body";
    /// `Http: Show Headers Tab` — default `<leader>h2`.
    pub const TAB_HEADERS: &str = "http.tab.headers";
    /// `Http: Show Timing Tab` — default `<leader>h3`.
    pub const TAB_TIMING: &str = "http.tab.timing";
    /// `Http: Show Raw Tab` — default `<leader>h4`.
    pub const TAB_RAW: &str = "http.tab.raw";
}

/// Default key chords for the M11.5 HTTP commands, as `(chord, command_id)`
/// pairs ready to feed `InputRouter::bind_extra_chord`. Kept here so the
/// binary doesn't hardcode chord strings — config layers (M11.5.x) can
/// override by re-binding the same command id.
pub fn default_keybindings() -> &'static [(&'static str, &'static str)] {
    &[
        ("Space h s", command_ids::SEND_REQUEST),
        ("Space h e", command_ids::SWITCH_ENVIRONMENT),
        ("Space h 1", command_ids::TAB_BODY),
        ("Space h 2", command_ids::TAB_HEADERS),
        ("Space h 3", command_ids::TAB_TIMING),
        ("Space h 4", command_ids::TAB_RAW),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use cockpit_http::{
        BasicAuth, BearerAuth, Body, FakeHttpEngine, HttpError, KeyValue, Meta, Request,
    };
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn env(name: &str, pairs: &[(&str, &str)]) -> Environment {
        let mut vars = BTreeMap::new();
        for (k, v) in pairs {
            vars.insert((*k).to_string(), (*v).to_string());
        }
        Environment {
            name: name.to_string(),
            vars,
        }
    }

    fn request(name: &str, url: &str) -> Request {
        let mut r = Request::empty();
        r.meta = Meta {
            name: Some(name.to_string()),
            seq: None,
            kind: None,
        };
        r.method = HttpMethod::Get;
        r.url = url.to_string();
        r
    }

    fn collection_with(requests: Vec<Request>, environments: Vec<Environment>) -> Collection {
        Collection {
            root: PathBuf::from("/tmp/test"),
            requests,
            environments,
        }
    }

    fn ok_response(body: &[u8], content_type: &str) -> Response {
        Response {
            status: 200,
            headers: vec![("content-type".into(), content_type.into())],
            body: body.to_vec(),
            elapsed: Duration::from_millis(42),
            redirects: Vec::new(),
            final_url: "https://example.com/".into(),
        }
    }

    #[test]
    fn new_auto_selects_first_request_and_first_environment() {
        let collection = collection_with(
            vec![request("a", "https://a"), request("b", "https://b")],
            vec![env("dev", &[("base", "x")]), env("prod", &[])],
        );
        let view = HttpView::new(collection);
        assert_eq!(view.selected_index(), Some(0));
        assert_eq!(view.active_environment_name(), Some("dev"));
        assert_eq!(view.response_tab(), ResponseTab::Body);
    }

    #[test]
    fn empty_collection_has_no_selection() {
        let view = HttpView::new(collection_with(Vec::new(), Vec::new()));
        assert_eq!(view.selected_index(), None);
        assert_eq!(view.active_environment_name(), None);
        assert!(view.selected_run().is_none());
    }

    #[test]
    fn select_next_saturates_at_the_last_request() {
        let mut view = HttpView::new(collection_with(
            vec![request("a", "https://a"), request("b", "https://b")],
            Vec::new(),
        ));
        view.select_next();
        assert_eq!(view.selected_index(), Some(1));
        view.select_next();
        assert_eq!(view.selected_index(), Some(1));
    }

    #[test]
    fn select_prev_saturates_at_zero() {
        let mut view = HttpView::new(collection_with(
            vec![request("a", "https://a"), request("b", "https://b")],
            Vec::new(),
        ));
        view.select_next();
        view.select_prev();
        assert_eq!(view.selected_index(), Some(0));
        view.select_prev();
        assert_eq!(view.selected_index(), Some(0));
    }

    #[test]
    fn switch_environment_to_unknown_name_reports_error() {
        let mut view = HttpView::new(collection_with(
            vec![request("a", "https://a")],
            vec![env("dev", &[])],
        ));
        let err = view.switch_environment(Some("prod")).unwrap_err();
        assert_eq!(err.name, "prod");
        // Active env unchanged on error.
        assert_eq!(view.active_environment_name(), Some("dev"));
    }

    #[test]
    fn switch_environment_to_none_clears_the_active() {
        let mut view = HttpView::new(collection_with(
            vec![request("a", "https://a")],
            vec![env("dev", &[])],
        ));
        view.switch_environment(None).unwrap();
        assert_eq!(view.active_environment_name(), None);
    }

    #[test]
    fn tab_cycle_wraps_in_display_order() {
        let mut view = HttpView::new(collection_with(vec![request("a", "https://a")], Vec::new()));
        assert_eq!(view.response_tab(), ResponseTab::Body);
        view.next_tab();
        assert_eq!(view.response_tab(), ResponseTab::Headers);
        view.next_tab();
        view.next_tab();
        assert_eq!(view.response_tab(), ResponseTab::Raw);
        view.next_tab();
        assert_eq!(view.response_tab(), ResponseTab::Body);
        view.prev_tab();
        assert_eq!(view.response_tab(), ResponseTab::Raw);
    }

    #[test]
    fn compute_split_clamps_request_to_at_least_one_pixel() {
        let mut view = HttpView::new(collection_with(vec![request("a", "https://a")], Vec::new()));
        view.set_split_ratio(0.0); // clamped to 0.15
        let split = view.compute_split(Rect::new(0, 0, 800, 600));
        assert!(split.request.height >= 1);
        assert_eq!(split.request.height + split.response.height, 600);
    }

    #[test]
    fn split_ratio_is_clamped_to_the_viable_band() {
        let mut view = HttpView::new(collection_with(vec![request("a", "https://a")], Vec::new()));
        view.set_split_ratio(0.01);
        assert!((view.split_ratio() - 0.15).abs() < 1e-6);
        view.set_split_ratio(0.99);
        assert!((view.split_ratio() - 0.85).abs() < 1e-6);
    }

    #[test]
    fn apply_result_sets_status_and_clears_prior_error() {
        let mut view = HttpView::new(collection_with(vec![request("a", "https://a")], Vec::new()));
        view.apply_result(Err(HttpError::Network("first try".into())));
        assert_eq!(view.selected_run().unwrap().status, SendStatus::Failed);
        view.apply_result(Ok(ok_response(b"{}", "application/json")));
        let run = view.selected_run().unwrap();
        assert_eq!(run.status, SendStatus::Ok);
        assert!(run.error.is_none());
        assert!(run.response.is_some());
    }

    #[test]
    fn cancelled_engine_result_lands_as_cancelled_status() {
        let mut view = HttpView::new(collection_with(vec![request("a", "https://a")], Vec::new()));
        view.mark_sending();
        view.apply_result(Err(HttpError::Cancelled));
        let run = view.selected_run().unwrap();
        assert_eq!(run.status, SendStatus::Cancelled);
        assert!(run.response.is_none());
        assert_eq!(run.error, Some(HttpError::Cancelled));
    }

    #[test]
    fn response_view_body_pretty_prints_json() {
        let mut view = HttpView::new(collection_with(vec![request("a", "https://a")], Vec::new()));
        view.apply_result(Ok(ok_response(
            br#"{"a":1,"b":[2,3],"c":"hi, world"}"#,
            "application/json",
        )));
        let ResponseView::Body { text, content_type } = view.response_view().unwrap() else {
            panic!("expected body view");
        };
        assert_eq!(content_type, "application/json");
        // Hand-rolled indenter: object opens onto its own line, comma
        // triggers newline, colons get a trailing space, string commas
        // are preserved.
        assert_eq!(
            text,
            "{\n  \"a\": 1,\n  \"b\": [\n    2,\n    3\n  ],\n  \"c\": \"hi, world\"\n}"
        );
    }

    #[test]
    fn response_view_body_passes_non_json_through_lossy() {
        let mut view = HttpView::new(collection_with(vec![request("a", "https://a")], Vec::new()));
        view.apply_result(Ok(ok_response(b"plain text body", "text/plain")));
        let ResponseView::Body { text, .. } = view.response_view().unwrap() else {
            panic!("expected body view");
        };
        assert_eq!(text, "plain text body");
    }

    #[test]
    fn response_view_headers_lists_in_declaration_order() {
        let mut view = HttpView::new(collection_with(vec![request("a", "https://a")], Vec::new()));
        view.apply_result(Ok(Response {
            headers: vec![
                ("x-trace".into(), "abc".into()),
                ("content-type".into(), "application/json".into()),
            ],
            ..ok_response(b"{}", "application/json")
        }));
        view.set_response_tab(ResponseTab::Headers);
        let ResponseView::Headers(rows) = view.response_view().unwrap() else {
            panic!("expected headers view");
        };
        assert_eq!(
            rows,
            vec![("x-trace", "abc"), ("content-type", "application/json"),]
        );
    }

    #[test]
    fn response_view_timing_exposes_elapsed_and_final_url() {
        let mut view = HttpView::new(collection_with(vec![request("a", "https://a")], Vec::new()));
        view.apply_result(Ok(Response {
            elapsed: Duration::from_millis(123),
            final_url: "https://api.example.com/v1/users".into(),
            ..ok_response(b"{}", "application/json")
        }));
        view.set_response_tab(ResponseTab::Timing);
        let ResponseView::Timing {
            elapsed,
            redirects,
            final_url,
        } = view.response_view().unwrap()
        else {
            panic!("expected timing view");
        };
        assert_eq!(elapsed, Duration::from_millis(123));
        assert_eq!(redirects, 0);
        assert_eq!(final_url, "https://api.example.com/v1/users");
    }

    #[test]
    fn response_view_raw_is_curl_v_shape() {
        let mut view = HttpView::new(collection_with(vec![request("a", "https://a")], Vec::new()));
        view.apply_result(Ok(ok_response(b"hello", "text/plain")));
        view.set_response_tab(ResponseTab::Raw);
        let ResponseView::Raw(text) = view.response_view().unwrap() else {
            panic!("expected raw view");
        };
        assert_eq!(text, "HTTP/1.1 200\ncontent-type: text/plain\n\nhello");
    }

    #[test]
    fn send_selected_drives_engine_and_records_response() {
        let collection = collection_with(
            vec![{
                let mut r = request("a", "{{base}}/users");
                r.headers = vec![KeyValue::new("X-Token", "{{tok}}")];
                r.auth = cockpit_http::Auth::Bearer(BearerAuth {
                    token: "{{tok}}".into(),
                });
                r
            }],
            vec![env(
                "dev",
                &[("base", "https://api.test"), ("tok", "secret")],
            )],
        );
        let mut view = HttpView::new(collection);
        let engine = FakeHttpEngine::new();
        engine.push_response(ok_response(b"{}", "application/json"));
        let cancel = cockpit_http::CancelHandle::new();

        let summary = send_selected(&mut view, &engine, &cancel).expect("send");
        assert_eq!(summary.method, HttpMethod::Get);
        assert_eq!(summary.url, "https://api.test/users");

        let run = view.selected_run().unwrap();
        assert_eq!(run.status, SendStatus::Ok);
        assert!(run.response.is_some());

        let sent = engine.requests();
        assert_eq!(sent.len(), 1);
        assert!(
            sent[0]
                .headers
                .iter()
                .any(|(k, v)| k == "Authorization" && v == "Bearer secret")
        );
    }

    #[test]
    fn send_selected_records_engine_failure_in_the_view() {
        let collection = collection_with(vec![request("a", "https://api.test/users")], Vec::new());
        let mut view = HttpView::new(collection);
        let engine = FakeHttpEngine::new();
        engine.push_error(HttpError::Timeout(Duration::from_secs(5)));
        let cancel = cockpit_http::CancelHandle::new();

        send_selected(&mut view, &engine, &cancel).expect("send");
        let run = view.selected_run().unwrap();
        assert_eq!(run.status, SendStatus::Failed);
        assert!(
            matches!(
                run.error.as_ref().unwrap(),
                HttpError::Timeout(d) if *d == Duration::from_secs(5)
            ),
            "unexpected error {:?}",
            run.error
        );
    }

    #[test]
    fn send_selected_reports_no_selection_for_empty_collection() {
        let mut view = HttpView::new(collection_with(Vec::new(), Vec::new()));
        let engine = FakeHttpEngine::new();
        let cancel = cockpit_http::CancelHandle::new();
        let err = send_selected(&mut view, &engine, &cancel).unwrap_err();
        assert!(matches!(err, SendError::NoSelection));
    }

    #[test]
    fn prepare_selected_surfaces_missing_var_error() {
        let collection =
            collection_with(vec![request("a", "{{base}}/users")], vec![env("dev", &[])]);
        let view = HttpView::new(collection);
        let err = prepare_selected(&view).unwrap_err();
        assert!(matches!(err, SendError::Prepare(_)), "got {err:?}");
    }

    #[test]
    fn basic_auth_request_round_trips_through_send_selected() {
        // Ensures the prepare/send pair handles Basic auth env interpolation
        // and that the fake engine sees the right Authorization header.
        let collection = collection_with(
            vec![{
                let mut r = request("a", "https://api.test/health");
                r.auth = cockpit_http::Auth::Basic(BasicAuth {
                    username: "{{user}}".into(),
                    password: "{{pass}}".into(),
                });
                r
            }],
            vec![env("dev", &[("user", "alice"), ("pass", "wonderland")])],
        );
        let mut view = HttpView::new(collection);
        let engine = FakeHttpEngine::new();
        engine.push_response(ok_response(b"", "text/plain"));
        let cancel = cockpit_http::CancelHandle::new();

        send_selected(&mut view, &engine, &cancel).expect("send");
        let sent = engine.requests();
        let auth = sent[0]
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(auth, "Basic YWxpY2U6d29uZGVybGFuZA==");
    }

    #[test]
    fn body_form_request_is_prepared_with_default_content_type() {
        let collection = collection_with(
            vec![{
                let mut r = request("a", "https://api.test/login");
                r.method = HttpMethod::Post;
                r.body = Body::Form(vec![KeyValue::new("name", "{{user}}")]);
                r
            }],
            vec![env("dev", &[("user", "alice")])],
        );
        let view = HttpView::new(collection);
        let prepared = prepare_selected(&view).unwrap();
        assert!(prepared.headers.iter().any(|(k, v)| {
            k.eq_ignore_ascii_case("content-type") && v == "application/x-www-form-urlencoded"
        }));
    }

    #[test]
    fn copy_selected_as_curl_uses_active_env() {
        let collection = collection_with(
            vec![{
                let mut r = request("a", "{{base}}/users");
                r.auth = cockpit_http::Auth::Bearer(BearerAuth {
                    token: "{{tok}}".into(),
                });
                r
            }],
            vec![env(
                "dev",
                &[("base", "https://api.test"), ("tok", "secret")],
            )],
        );
        let view = HttpView::new(collection);
        let curl = copy_selected_as_curl(&view).unwrap();
        assert!(curl.starts_with("curl \\\n  'https://api.test/users'"));
        assert!(curl.contains("-H 'Authorization: Bearer secret'"));
    }

    #[test]
    fn copy_selected_as_curl_propagates_no_selection() {
        let view = HttpView::new(collection_with(Vec::new(), Vec::new()));
        let err = copy_selected_as_curl(&view).unwrap_err();
        assert!(matches!(err, SendError::NoSelection));
    }

    #[test]
    fn default_keybindings_cover_send_environment_and_every_tab() {
        let bindings = default_keybindings();
        let ids: Vec<&str> = bindings.iter().map(|(_, id)| *id).collect();
        assert!(ids.contains(&command_ids::SEND_REQUEST));
        assert!(ids.contains(&command_ids::SWITCH_ENVIRONMENT));
        for tab in [
            command_ids::TAB_BODY,
            command_ids::TAB_HEADERS,
            command_ids::TAB_TIMING,
            command_ids::TAB_RAW,
        ] {
            assert!(ids.contains(&tab), "missing chord for {tab}");
        }
    }
}
