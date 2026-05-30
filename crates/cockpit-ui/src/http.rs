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

/// Vertical thickness (logical px) of the draggable divider band straddling
/// the request/response boundary. Mirrors the 6 px pane-border hit band the
/// workspace splitter uses (binary `detect_border_drag`, M4.7) so the HTTP
/// view's split feels identical to dragging a side pane.
pub const SPLIT_HANDLE_THICKNESS: u32 = 6;

/// Horizontal padding, in cells, on each side of a tab label in the response
/// tab strip. The strip is monospaced (terminal-first), so a tab's pixel
/// width is `(label.chars().count() + 2 * TAB_PADDING_CELLS) * cell_width`.
pub const TAB_PADDING_CELLS: u32 = 1;

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

    /// Replace the vars on the environment named `name`, in-place
    /// (v0.11 M11.6.2). Returns `Err` if no such environment exists —
    /// callers should validate the name was real at send time and bail
    /// here only on a genuine race (e.g. the user reloaded the
    /// collection mid-flight).
    ///
    /// The mutation is *in-memory only*. Persisting back to the on-disk
    /// `environments/<name>.bru` happens through the M11.4.1 editor
    /// surface — we keep that boundary clean so a single typo in a
    /// post-response script doesn't corrupt the file on disk.
    pub fn replace_environment_vars(
        &mut self,
        name: &str,
        vars: std::collections::BTreeMap<String, String>,
    ) -> Result<(), UnknownEnvironment> {
        match self
            .collection
            .environments
            .iter_mut()
            .find(|env| env.name == name)
        {
            Some(env) => {
                env.vars = vars;
                Ok(())
            }
            None => Err(UnknownEnvironment {
                name: name.to_string(),
            }),
        }
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
        let boundary = viewport.y + request_height;
        // Centre the drag band on the boundary, clamped inside the viewport so
        // the band never spills past either edge on a tiny pane.
        let half = SPLIT_HANDLE_THICKNESS / 2;
        let handle_top = boundary.saturating_sub(half).max(viewport.y);
        let handle_bottom =
            (boundary + (SPLIT_HANDLE_THICKNESS - half)).min(viewport.y + viewport.height);
        SplitLayout {
            request: Rect::new(viewport.x, viewport.y, viewport.width, request_height),
            response: Rect::new(
                viewport.x,
                viewport.y + request_height,
                viewport.width,
                response_height,
            ),
            handle: Rect::new(
                viewport.x,
                handle_top,
                viewport.width,
                handle_bottom.saturating_sub(handle_top),
            ),
        }
    }

    /// Map a pointer `y` (logical px) inside `viewport` to a split ratio and
    /// apply it (clamped to the viable band). Returns the applied ratio. The
    /// painter calls this while a [`SplitLayout::handle`] drag is in flight.
    pub fn drag_split_to(&mut self, viewport: Rect, pointer_y: f32) -> f32 {
        let span = viewport.height.max(1) as f32;
        let ratio = (pointer_y - viewport.y as f32) / span;
        self.set_split_ratio(ratio);
        self.split_ratio
    }

    /// Lay out the response-pane tab strip: one clickable rectangle per
    /// [`ResponseTab`], left-to-right across the top row of `response_pane`.
    /// `cell_width` / `row_height` are the painter's monospaced glyph metrics
    /// (logical px); the strip is exactly one row tall.
    pub fn tab_strip(&self, response_pane: Rect, cell_width: u32, row_height: u32) -> TabStrip {
        let mut x = response_pane.x;
        let y = response_pane.y;
        let height = row_height.min(response_pane.height);
        let tabs = ResponseTab::all().map(|tab| {
            let cells = tab.label().chars().count() as u32 + 2 * TAB_PADDING_CELLS;
            let width = cells * cell_width;
            let rect = Rect::new(x, y, width, height);
            x = x.saturating_add(width);
            (tab, rect)
        });
        TabStrip {
            tabs,
            active: self.tab,
        }
    }

    /// Hit-test a pointer against the response tab strip and, on a hit,
    /// activate that tab. Returns the newly-active tab when the click landed
    /// on one, or `None` when it missed the strip. The painter's single mouse
    /// entry point for the response panel header.
    pub fn click_response_tab(
        &mut self,
        response_pane: Rect,
        cell_width: u32,
        row_height: u32,
        x: f32,
        y: f32,
    ) -> Option<ResponseTab> {
        let hit = self
            .tab_strip(response_pane, cell_width, row_height)
            .hit(x, y)?;
        self.set_response_tab(hit);
        Some(hit)
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
    /// Draggable divider band straddling the request/response boundary
    /// ([`SPLIT_HANDLE_THICKNESS`] tall). The painter hit-tests this to start
    /// a resize drag, then feeds the pointer into [`HttpView::drag_split_to`].
    pub handle: Rect,
}

impl SplitLayout {
    /// True when the logical-pixel pointer `(x, y)` falls in the divider band.
    pub fn handle_contains(&self, x: f32, y: f32) -> bool {
        rect_contains(self.handle, x, y)
    }
}

/// Response-pane tab strip layout: one clickable rectangle per
/// [`ResponseTab`] laid left-to-right across the top row. Produced by
/// [`HttpView::tab_strip`]; the painter draws each rect and routes clicks
/// through [`TabStrip::hit`] (or [`HttpView::click_response_tab`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabStrip {
    /// Each tab paired with its on-screen rectangle, in display order.
    pub tabs: [(ResponseTab, Rect); 4],
    /// The currently-active tab, so the painter can highlight it without a
    /// second call into the view.
    pub active: ResponseTab,
}

impl TabStrip {
    /// Which tab, if any, contains the logical-pixel pointer `(x, y)`.
    pub fn hit(&self, x: f32, y: f32) -> Option<ResponseTab> {
        self.tabs
            .iter()
            .find(|(_, rect)| rect_contains(*rect, x, y))
            .map(|(tab, _)| *tab)
    }
}

/// Point-in-rectangle test for an `f32` logical pointer against a `u32`
/// rectangle. Right/bottom edges are exclusive so adjacent tab rectangles
/// never both claim the same pixel column.
fn rect_contains(rect: Rect, x: f32, y: f32) -> bool {
    let left = rect.x as f32;
    let top = rect.y as f32;
    let right = (rect.x + rect.width) as f32;
    let bottom = (rect.y + rect.height) as f32;
    x >= left && x < right && y >= top && y < bottom
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
    /// `Http: Cancel In-flight Request` — trips the cancel handle on the
    /// most recent send. No-op if nothing is in flight.
    pub const CANCEL: &str = "http.cancel";
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

/// Surface script-related warnings for the currently selected request
/// (v0.11 M11.6). Returned strings are user-facing one-liners ready to
/// drop into the status bar. Used by the binary on `Http: Send Request`
/// so the user is told once per send that:
///
/// - The request carries Bruno-style JS scripts that cockpit will not run.
/// - The request carries cockpit Lua scripts but the `http.scripts`
///   capability has not been granted (default-deny). The flag is set by
///   the caller because the capability set lives in `cockpit-lua`,
///   which `cockpit-ui` does not depend on — we keep the policy decision
///   in the binary and let this helper just format the message.
///
/// Returns an empty `Vec` when the request has no scripts or all gates
/// pass.
pub fn script_warnings(view: &HttpView, http_scripts_granted: bool) -> Vec<String> {
    let mut warnings = Vec::new();
    let Some(req) = view.selected_request() else {
        return warnings;
    };
    if req.has_js_scripts {
        warnings.push("Lua scripting only; JS scripts skipped. See docs/http.md.".to_string());
    }
    let has_lua = req.pre_script.is_some() || req.post_script.is_some();
    if has_lua && !http_scripts_granted {
        warnings.push(
            "Lua scripts present but `http.scripts` capability not granted; scripts skipped."
                .to_string(),
        );
    }
    warnings
}

/// Default key chords for the M11.5 HTTP commands, as `(chord, command_id)`
/// pairs ready to feed `InputRouter::bind_extra_chord`. Kept here so the
/// binary doesn't hardcode chord strings — config layers (M11.5.x) can
/// override by re-binding the same command id. The `<leader>` prefix is
/// substituted with the configured leader key by the binary, matching the
/// tool-recipe convention so HTTP binds follow a rebound leader.
pub fn default_keybindings() -> &'static [(&'static str, &'static str)] {
    &[
        ("<leader>hs", command_ids::SEND_REQUEST),
        ("<leader>he", command_ids::SWITCH_ENVIRONMENT),
        ("<leader>h1", command_ids::TAB_BODY),
        ("<leader>h2", command_ids::TAB_HEADERS),
        ("<leader>h3", command_ids::TAB_TIMING),
        ("<leader>h4", command_ids::TAB_RAW),
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
    fn replace_environment_vars_swaps_the_named_env_in_place() {
        let mut view = HttpView::new(collection_with(
            vec![request("a", "https://a")],
            vec![env("dev", &[("base", "https://old")])],
        ));
        let mut new_vars = std::collections::BTreeMap::new();
        new_vars.insert("base".into(), "https://new".into());
        new_vars.insert("token".into(), "fresh".into());
        view.replace_environment_vars("dev", new_vars).unwrap();
        let env = view.active_environment().expect("env");
        assert_eq!(
            env.vars.get("base").map(String::as_str),
            Some("https://new")
        );
        assert_eq!(env.vars.get("token").map(String::as_str), Some("fresh"));
    }

    #[test]
    fn replace_environment_vars_errors_on_unknown_name() {
        let mut view = HttpView::new(collection_with(
            vec![request("a", "https://a")],
            vec![env("dev", &[])],
        ));
        let err = view
            .replace_environment_vars("staging", Default::default())
            .unwrap_err();
        assert_eq!(err.name, "staging");
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
    fn compute_split_centres_the_handle_band_on_the_boundary() {
        let view = HttpView::new(collection_with(vec![request("a", "https://a")], Vec::new()));
        // Default 0.5 ratio of a 600px-tall pane → boundary at 300.
        let split = view.compute_split(Rect::new(0, 0, 800, 600));
        let boundary = split.request.height; // y of the response top
        assert_eq!(boundary, split.response.y);
        assert_eq!(split.handle.height, SPLIT_HANDLE_THICKNESS);
        assert_eq!(split.handle.width, 800);
        // Band straddles the boundary.
        assert!(split.handle.y <= boundary);
        assert!(split.handle.y + split.handle.height >= boundary);
    }

    #[test]
    fn handle_contains_matches_the_drag_band() {
        let view = HttpView::new(collection_with(vec![request("a", "https://a")], Vec::new()));
        let split = view.compute_split(Rect::new(0, 0, 800, 600));
        let boundary = split.request.height as f32;
        assert!(split.handle_contains(400.0, boundary));
        // Well above the boundary is in the request body, not the handle.
        assert!(!split.handle_contains(400.0, boundary - 50.0));
    }

    #[test]
    fn drag_split_to_maps_pointer_y_to_a_clamped_ratio() {
        let mut view = HttpView::new(collection_with(vec![request("a", "https://a")], Vec::new()));
        let viewport = Rect::new(0, 0, 800, 600);
        // Drag to 25% down the pane.
        let ratio = view.drag_split_to(viewport, 150.0);
        assert!((ratio - 0.25).abs() < 1e-6);
        // Dragging past the top clamps to the viable floor (0.15).
        let clamped = view.drag_split_to(viewport, 0.0);
        assert!((clamped - 0.15).abs() < 1e-6);
    }

    #[test]
    fn tab_strip_lays_tabs_left_to_right_without_gaps() {
        let view = HttpView::new(collection_with(vec![request("a", "https://a")], Vec::new()));
        let pane = Rect::new(10, 200, 800, 400);
        let strip = view.tab_strip(pane, 8, 16);
        assert_eq!(strip.active, ResponseTab::Body);
        // First tab starts flush with the pane's left edge / top.
        assert_eq!(strip.tabs[0].1.x, 10);
        assert_eq!(strip.tabs[0].1.y, 200);
        assert_eq!(strip.tabs[0].1.height, 16);
        // "Body" = 4 chars + 2 padding cells = 6 cells * 8px = 48px wide.
        assert_eq!(strip.tabs[0].1.width, 48);
        // Tabs butt up against each other with no overlap or gap.
        for pair in strip.tabs.windows(2) {
            let (_, prev) = pair[0];
            let (_, next) = pair[1];
            assert_eq!(prev.x + prev.width, next.x);
        }
    }

    #[test]
    fn click_response_tab_activates_the_hit_tab() {
        let mut view = HttpView::new(collection_with(vec![request("a", "https://a")], Vec::new()));
        let pane = Rect::new(0, 0, 800, 400);
        let strip = view.tab_strip(pane, 8, 16);
        // Click in the middle of the Headers tab.
        let headers_rect = strip.tabs[1].1;
        let cx = (headers_rect.x + headers_rect.width / 2) as f32;
        let cy = (headers_rect.y + headers_rect.height / 2) as f32;
        let hit = view.click_response_tab(pane, 8, 16, cx, cy);
        assert_eq!(hit, Some(ResponseTab::Headers));
        assert_eq!(view.response_tab(), ResponseTab::Headers);
    }

    #[test]
    fn click_below_the_strip_is_a_miss_and_leaves_the_tab_unchanged() {
        let mut view = HttpView::new(collection_with(vec![request("a", "https://a")], Vec::new()));
        let pane = Rect::new(0, 0, 800, 400);
        // y far below the one-row strip.
        let hit = view.click_response_tab(pane, 8, 16, 20.0, 300.0);
        assert_eq!(hit, None);
        assert_eq!(view.response_tab(), ResponseTab::Body);
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
    fn script_warnings_flags_js_scripts_with_a_toast() {
        let collection = collection_with(
            vec![{
                let mut r = request("a", "https://x");
                r.has_js_scripts = true;
                r
            }],
            Vec::new(),
        );
        let view = HttpView::new(collection);
        let warnings = script_warnings(&view, true);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("JS scripts skipped"));
    }

    #[test]
    fn script_warnings_flags_ungranted_lua_scripts() {
        let collection = collection_with(
            vec![{
                let mut r = request("a", "https://x");
                r.pre_script = Some("cockpit.http.set_var('x', '1')".into());
                r
            }],
            Vec::new(),
        );
        let view = HttpView::new(collection);
        let warnings = script_warnings(&view, false);
        assert!(
            warnings.iter().any(|w| w.contains("`http.scripts`")),
            "{:?}",
            warnings
        );
    }

    #[test]
    fn script_warnings_is_empty_when_no_scripts_present() {
        let collection = collection_with(vec![request("a", "https://x")], Vec::new());
        let view = HttpView::new(collection);
        let warnings = script_warnings(&view, false);
        assert!(warnings.is_empty());
    }

    #[test]
    fn script_warnings_omits_lua_warning_when_capability_granted() {
        let collection = collection_with(
            vec![{
                let mut r = request("a", "https://x");
                r.post_script = Some("-- log".into());
                r
            }],
            Vec::new(),
        );
        let view = HttpView::new(collection);
        let warnings = script_warnings(&view, true);
        assert!(
            warnings.iter().all(|w| !w.contains("`http.scripts`")),
            "{:?}",
            warnings
        );
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
