use crate::{
    cef_layer::{
        cancel_creation_on_ui, close_browser, create_tab_on_ui, post_ui, take_profile, take_tab,
        take_tabs_matching,
        PendingCdp, SharedState,
    },
    snapshot::render_ax_tree,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use browser_protocol::{valid_profile_name, Action, Page, Reply, Request, TabInfo};
use cef::{ImplBrowser, ImplBrowserHost};
use serde_json::{json, Value};
use std::{
    fs,
    sync::{
        atomic::{AtomicI32, Ordering},
        OnceLock,
    },
    time::Duration,
};
use tokio::{
    sync::{oneshot, Mutex as AsyncMutex},
    time::sleep,
};

const UI_TIMEOUT: Duration = Duration::from_secs(10);
const CDP_TIMEOUT: Duration = Duration::from_secs(30);
const LOAD_TIMEOUT: Duration = Duration::from_secs(30);

static NEXT_CDP_ID: AtomicI32 = AtomicI32::new(1);
static CREATION_LOCK: OnceLock<AsyncMutex<()>> = OnceLock::new();

fn creation_lock() -> &'static AsyncMutex<()> {
    CREATION_LOCK.get_or_init(|| AsyncMutex::new(()))
}

pub async fn handle_request(shared: SharedState, request: Request) -> Result<Reply, String> {
    match request {
        Request::Open { profile, owner, url } => open(shared, profile, owner, url).await,
        Request::Navigate { tab, url } => {
            navigate(&shared, &tab, url, LOAD_TIMEOUT).await.map(|page| Reply::Navigated { page })
        }
        Request::Snapshot { tab } => snapshot(shared, tab).await,
        Request::Act { tab, element, action } => act(shared, tab, element, action).await,
        Request::Evaluate { tab, expression } => evaluate(&shared, &tab, &expression)
            .await
            .map(|value| Reply::Evaluated { value }),
        Request::Screenshot { tab, full_page } => screenshot(shared, tab, full_page).await,
        Request::Tabs { owner } => tabs(shared, owner).await,
        Request::Close { tab } => close(shared, tab).await,
        Request::CloseOwner { owner } => close_owner(shared, owner).await,
        Request::DeleteProfile { profile } => delete_profile(shared, profile).await,
    }
}

async fn ui_call<T>(job: impl FnOnce() -> T + Send + 'static) -> Result<T, String>
where
    T: Send + 'static,
{
    let (sender, receiver) = oneshot::channel();
    post_ui(move || {
        let _ = sender.send(job());
    })?;
    let receiver = tokio::time::timeout(UI_TIMEOUT, receiver)
        .await
        .map_err(|_| "timed out waiting for the CEF UI thread".to_owned())?;
    receiver
        .map_err(|_| "CEF UI thread finished without answering".to_owned())
}

fn tab_missing(tab: &str) -> String {
    format!("unknown tab '{tab}'")
}

async fn cdp(
    shared: &SharedState,
    tab: &str,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let message_id = NEXT_CDP_ID.fetch_add(1, Ordering::Relaxed);
    let (sender, receiver) = oneshot::channel();
    let tab_name = tab.to_owned();
    let method_name = method.to_owned();
    let shared_for_dispatch = shared.clone();

    ui_call(move || {
        let host = {
            let mut state = shared_for_dispatch.lock().expect("browser state poisoned");
            let Some(tab_state) = state.tabs.get(&tab_name) else {
                return Err(tab_missing(&tab_name));
            };
            let Some(host) = tab_state.browser.host() else {
                return Err(format!("tab '{tab_name}' has no browser host"));
            };
            state.pending_cdp.insert(
                message_id,
                PendingCdp {
                    tab: tab_name.clone(),
                    sender,
                },
            );
            host
        };

        let message = json!({
            "id": message_id,
            "method": method_name,
            "params": params,
        })
        .to_string();
        if host.send_dev_tools_message(Some(message.as_bytes())) == 0 {
            let pending = shared_for_dispatch
                .lock()
                .expect("browser state poisoned")
                .pending_cdp
                .remove(&message_id);
            drop(pending);
            Err("failed to submit CDP message".to_owned())
        } else {
            Ok(())
        }
    })
    .await??;

    match tokio::time::timeout(CDP_TIMEOUT, receiver).await {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(_)) => Err(format!("CDP {method} finished without a reply")),
        Err(_) => {
            let shared_for_cleanup = shared.clone();
            let _ = post_ui(move || {
                shared_for_cleanup
                    .lock()
                    .expect("browser state poisoned")
                    .pending_cdp
                    .remove(&message_id);
            });
            Err(format!("CDP {method} timed out"))
        }
    }
}

async fn load_position(shared: &SharedState, tab: &str) -> Result<(i32, u64), String> {
    let tab = tab.to_owned();
    let shared_for_call = shared.clone();
    ui_call(move || {
        let state = shared_for_call.lock().expect("browser state poisoned");
        let Some(tab_state) = state.tabs.get(&tab) else {
            return Err(tab_missing(&tab));
        };
        Ok((tab_state.browser_id, state.load_seq))
    })
    .await?
}

async fn prepare_navigation(shared: &SharedState, tab: &str) -> Result<(i32, u64), String> {
    let tab = tab.to_owned();
    let shared_for_call = shared.clone();
    ui_call(move || {
        let mut state = shared_for_call.lock().expect("browser state poisoned");
        let Some(tab_state) = state.tabs.get_mut(&tab) else {
            return Err(tab_missing(&tab));
        };
        tab_state.refs.clear();
        Ok((tab_state.browser_id, state.load_seq))
    })
    .await?
}

async fn wait_for_load(
    shared: &SharedState,
    browser_id: i32,
    since: u64,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let shared_for_call = shared.clone();
        let status = ui_call(move || {
            let state = shared_for_call.lock().expect("browser state poisoned");
            let Some(tab_state) = state.tabs.values().find(|tab| tab.browser_id == browser_id) else {
                return Err("tab closed while waiting for the page load".to_owned());
            };
            let load_error = state
                .load_errors
                .get(&browser_id)
                .filter(|(sequence, _)| *sequence > since)
                .map(|(_, error)| error.clone());
            let end_sequence = state.load_ends.get(&browser_id).copied().unwrap_or(0);
            Ok((
                end_sequence,
                tab_state.browser.is_loading() != 0,
                load_error,
            ))
        })
        .await??;

        let (end_sequence, is_loading, load_error) = status;
        if let Some(error) = load_error {
            return Err(error);
        }
        if end_sequence > since && !is_loading {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("timed out waiting for the page load".to_owned());
        }
        sleep(Duration::from_millis(100)).await;
    }
}

fn exception_message(details: &Value) -> String {
    details
        .get("exception")
        .and_then(|exception| exception.get("description"))
        .and_then(Value::as_str)
        .or_else(|| details.get("text").and_then(Value::as_str))
        .unwrap_or("unknown JavaScript exception")
        .to_owned()
}

fn evaluation_value(response: Value) -> Result<Value, String> {
    if let Some(details) = response.get("exceptionDetails") {
        return Err(format!("JavaScript exception: {}", exception_message(details)));
    }
    let Some(remote) = response.get("result") else {
        return Err("Runtime evaluation returned no result".to_owned());
    };
    if let Some(value) = remote.get("value") {
        return Ok(value.clone());
    }
    if let Some(description) = remote.get("description").and_then(Value::as_str) {
        return Ok(Value::String(description.to_owned()));
    }
    Ok(Value::Null)
}

async fn evaluate(shared: &SharedState, tab: &str, expression: &str) -> Result<Value, String> {
    let response = cdp(
        shared,
        tab,
        "Runtime.evaluate",
        json!({
            "expression": expression,
            "awaitPromise": true,
            "returnByValue": true,
        }),
    )
    .await?;
    evaluation_value(response)
}

async fn refresh_page(shared: &SharedState, tab: &str) -> Result<Page, String> {
    let value = evaluate(
        shared,
        tab,
        "({url: window.location.href, title: document.title})",
    )
    .await?;
    let page = Page {
        url: value
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    };
    let tab_name = tab.to_owned();
    let page_for_state = page.clone();
    let shared_for_call = shared.clone();
    ui_call(move || {
        if let Some(tab_state) = shared_for_call
            .lock()
            .expect("browser state poisoned")
            .tabs
            .get_mut(&tab_name)
        {
            tab_state.page = page_for_state;
        }
    })
    .await?;
    Ok(page)
}

async fn navigate(
    shared: &SharedState,
    tab: &str,
    url: String,
    timeout: Duration,
) -> Result<Page, String> {
    let (browser_id, since) = prepare_navigation(shared, tab).await?;
    let result = cdp(shared, tab, "Page.navigate", json!({ "url": url })).await?;
    if let Some(error) = result.get("errorText").and_then(Value::as_str) {
        return Err(format!("navigation failed: {error}"));
    }
    wait_for_load(shared, browser_id, since, timeout).await?;
    refresh_page(shared, tab).await
}

async fn open(
    shared: SharedState,
    profile: String,
    owner: String,
    url: Option<String>,
) -> Result<Reply, String> {
    if !valid_profile_name(&profile) {
        return Err(format!("invalid profile name '{profile}'"));
    }
    // Browser creation is correlated through CEF's creation callback order;
    // serialize it so concurrent Open requests cannot exchange identities.
    let _creation_permit = creation_lock().lock().await;
    let shared_for_create = shared.clone();
    let profile_for_create = profile.clone();
    let owner_for_create = owner.clone();
    let (tab, creation) = ui_call(move || {
        create_tab_on_ui(&shared_for_create, &profile_for_create, &owner_for_create)
    })
    .await??;

    let created = match tokio::time::timeout(Duration::from_secs(30), creation).await {
        Ok(Ok(created)) => created,
        Ok(Err(_)) => Err("CEF browser creation ended without a reply".to_owned()),
        Err(_) => {
            let shared_for_cancel = shared.clone();
            let tab_for_cancel = tab.clone();
            let cancelled = ui_call(move || {
                cancel_creation_on_ui(&shared_for_cancel, &tab_for_cancel)
            })
            .await
            .unwrap_or(false);
            if !cancelled {
                let _ = close(shared.clone(), tab.clone()).await;
            }
            Err("timed out waiting for browser creation".to_owned())
        }
    };
    if let Err(error) = created {
        return Err(error);
    }
    drop(_creation_permit);

    let result = match url {
        Some(url) => navigate(&shared, &tab, url, LOAD_TIMEOUT).await,
        None => refresh_page(&shared, &tab).await,
    };
    match result {
        Ok(page) => Ok(Reply::Opened { tab, page }),
        Err(error) => {
            let _ = close(shared.clone(), tab).await;
            Err(error)
        }
    }
}

async fn snapshot(shared: SharedState, tab: String) -> Result<Reply, String> {
    let page = refresh_page(&shared, &tab).await?;
    let result = cdp(&shared, &tab, "Accessibility.getFullAXTree", json!({})).await?;
    let (tree, refs) = render_ax_tree(&result)?;
    let tab_name = tab.clone();
    let shared_for_call = shared.clone();
    ui_call(move || {
        let mut state = shared_for_call.lock().expect("browser state poisoned");
        let Some(tab_state) = state.tabs.get_mut(&tab_name) else {
            return Err(tab_missing(&tab_name));
        };
        tab_state.refs = refs;
        Ok(())
    })
    .await??;
    Ok(Reply::Snapshot { page, tree })
}

async fn resolve_ref(shared: &SharedState, tab: &str, reference: &str) -> Result<i64, String> {
    let tab = tab.to_owned();
    let reference = reference.to_owned();
    let shared_for_call = shared.clone();
    ui_call(move || {
        let state = shared_for_call.lock().expect("browser state poisoned");
        let Some(tab_state) = state.tabs.get(&tab) else {
            return Err(tab_missing(&tab));
        };
        tab_state
            .refs
            .get(&reference)
            .copied()
            .ok_or_else(|| format!("unknown ref '{reference}'; take a new snapshot"))
    })
    .await?
}

async fn scroll_and_center(
    shared: &SharedState,
    tab: &str,
    backend_node: i64,
    reference: &str,
) -> Result<(f64, f64), String> {
    cdp(
        shared,
        tab,
        "DOM.scrollIntoViewIfNeeded",
        json!({ "backendNodeId": backend_node }),
    )
    .await
    .map_err(|error| format!("cannot use ref '{reference}'; take a new snapshot ({error})"))?;
    let model = cdp(
        shared,
        tab,
        "DOM.getBoxModel",
        json!({ "backendNodeId": backend_node }),
    )
    .await
    .map_err(|error| format!("cannot use ref '{reference}'; take a new snapshot ({error})"))?;
    let content = model
        .get("model")
        .and_then(|model| model.get("content"))
        .and_then(Value::as_array)
        .ok_or_else(|| format!("element '{reference}' has no layout box"))?;
    if content.len() < 8 {
        return Err(format!("element '{reference}' has no layout box"));
    }
    let numbers = content
        .iter()
        .take(8)
        .map(Value::as_f64)
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| format!("element '{reference}' has an invalid layout box"))?;
    let x = (numbers[0] + numbers[2] + numbers[4] + numbers[6]) / 4.0;
    let y = (numbers[1] + numbers[3] + numbers[5] + numbers[7]) / 4.0;
    Ok((x, y))
}

async fn mouse_event(
    shared: &SharedState,
    tab: &str,
    kind: &str,
    x: f64,
    y: f64,
    button: &str,
    buttons: i32,
    click_count: i32,
) -> Result<(), String> {
    cdp(
        shared,
        tab,
        "Input.dispatchMouseEvent",
        json!({
            "type": kind,
            "x": x,
            "y": y,
            "button": button,
            "buttons": buttons,
            "clickCount": click_count,
        }),
    )
    .await?;
    Ok(())
}

struct KeyDef {
    key: String,
    code: String,
    windows_code: i32,
    text: Option<String>,
}

fn key_def(key: &str) -> Option<KeyDef> {
    let simple = |key: &str, code: &str, windows_code: i32, text: Option<&str>| KeyDef {
        key: key.to_owned(),
        code: code.to_owned(),
        windows_code,
        text: text.map(str::to_owned),
    };
    Some(match key {
        "Enter" => simple("Enter", "Enter", 13, Some("\r")),
        "Tab" => simple("Tab", "Tab", 9, None),
        "Escape" | "Esc" => simple("Escape", "Escape", 27, None),
        "Backspace" => simple("Backspace", "Backspace", 8, None),
        "Delete" => simple("Delete", "Delete", 46, None),
        "ArrowDown" => simple("ArrowDown", "ArrowDown", 40, None),
        "ArrowLeft" => simple("ArrowLeft", "ArrowLeft", 37, None),
        "ArrowRight" => simple("ArrowRight", "ArrowRight", 39, None),
        "ArrowUp" => simple("ArrowUp", "ArrowUp", 38, None),
        "Home" => simple("Home", "Home", 36, None),
        "End" => simple("End", "End", 35, None),
        "PageDown" => simple("PageDown", "PageDown", 34, None),
        "PageUp" => simple("PageUp", "PageUp", 33, None),
        "Space" | " " => simple(" ", "Space", 32, Some(" ")),
        _ if key.chars().count() == 1 => {
            let character = key.chars().next().expect("single-character key");
            if !character.is_ascii_alphanumeric() {
                return None;
            }
            let upper = character.to_ascii_uppercase();
            let code = if character.is_ascii_digit() {
                format!("Digit{upper}")
            } else {
                format!("Key{upper}")
            };
            KeyDef {
                key: character.to_string(),
                code,
                windows_code: upper as i32,
                text: Some(character.to_string()),
            }
        }
        _ => return None,
    })
}

async fn send_key(shared: &SharedState, tab: &str, key: &str) -> Result<(), String> {
    let definition = key_def(key)
        .ok_or_else(|| format!("unsupported key '{key}' (expected a common key such as Enter, Tab, Escape, ArrowDown)"))?;
    let down_type = if definition.text.is_some() { "keyDown" } else { "rawKeyDown" };
    for (kind, include_text) in [(down_type, true), ("keyUp", false)] {
        let mut params = json!({
            "type": kind,
            "key": definition.key,
            "code": definition.code,
            "windowsVirtualKeyCode": definition.windows_code,
        });
        if include_text {
            if let Some(text) = &definition.text {
                params["text"] = json!(text);
                params["unmodifiedText"] = json!(text);
            }
        }
        cdp(shared, tab, "Input.dispatchKeyEvent", params).await?;
    }
    Ok(())
}

async fn resolve_object(
    shared: &SharedState,
    tab: &str,
    backend_node: i64,
) -> Result<String, String> {
    let result = cdp(
        shared,
        tab,
        "DOM.resolveNode",
        json!({ "backendNodeId": backend_node }),
    )
    .await?;
    result
        .get("object")
        .and_then(|object| object.get("objectId"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "DOM.resolveNode returned no object id".to_owned())
}

async fn call_function_on(
    shared: &SharedState,
    tab: &str,
    object_id: &str,
    function: &str,
    arguments: Vec<Value>,
) -> Result<Value, String> {
    let response = cdp(
        shared,
        tab,
        "Runtime.callFunctionOn",
        json!({
            "objectId": object_id,
            "functionDeclaration": function,
            "arguments": arguments,
            "awaitPromise": true,
            "returnByValue": true,
        }),
    )
    .await?;
    evaluation_value(response)
}

async fn focus_and_select(
    shared: &SharedState,
    tab: &str,
    backend_node: i64,
) -> Result<(), String> {
    let object = resolve_object(shared, tab, backend_node).await?;
    call_function_on(
        shared,
        tab,
        &object,
        r#"function() {
            this.focus();
            if (this.select) {
                this.select();
            } else if (this.isContentEditable) {
                const range = document.createRange();
                range.selectNodeContents(this);
                const selection = window.getSelection();
                selection.removeAllRanges();
                selection.addRange(range);
            }
        }"#,
        Vec::new(),
    )
    .await?;
    Ok(())
}

async fn settle_after_action(shared: &SharedState, browser_id: i32, since: u64) {
    sleep(Duration::from_millis(350)).await;
    let started = tokio::time::Instant::now();
    loop {
        let shared_for_call = shared.clone();
        let status = ui_call(move || {
            let state = shared_for_call.lock().expect("browser state poisoned");
            let Some(tab_state) = state.tabs.values().find(|tab| tab.browser_id == browser_id) else {
                return (0, false, false);
            };
            (
                state.load_ends.get(&browser_id).copied().unwrap_or(0),
                tab_state.browser.is_loading() != 0,
                true,
            )
        })
        .await;
        let Ok((end_sequence, is_loading, exists)) = status else {
            return;
        };
        if !exists || (end_sequence > since && !is_loading) {
            return;
        }
        if end_sequence <= since && !is_loading && started.elapsed() > Duration::from_millis(500) {
            return;
        }
        if started.elapsed() > Duration::from_secs(5) {
            return;
        }
        sleep(Duration::from_millis(100)).await;
    }
}

async fn act(
    shared: SharedState,
    tab: String,
    reference: String,
    action: Action,
) -> Result<Reply, String> {
    let backend_node = resolve_ref(&shared, &tab, &reference).await?;
    let (browser_id, since) = load_position(&shared, &tab).await?;
    let (x, y) = scroll_and_center(&shared, &tab, backend_node, &reference).await?;

    match action {
        Action::Click => {
            mouse_event(&shared, &tab, "mouseMoved", x, y, "none", 0, 0).await?;
            mouse_event(&shared, &tab, "mousePressed", x, y, "left", 1, 1).await?;
            mouse_event(&shared, &tab, "mouseReleased", x, y, "left", 0, 1).await?;
        }
        Action::Hover => {
            mouse_event(&shared, &tab, "mouseMoved", x, y, "none", 0, 0).await?;
        }
        Action::Type { text, submit } => {
            focus_and_select(&shared, &tab, backend_node).await?;
            send_key(&shared, &tab, "Delete").await?;
            cdp(&shared, &tab, "Input.insertText", json!({ "text": text })).await?;
            if submit {
                send_key(&shared, &tab, "Enter").await?;
            }
        }
        Action::Press { key } => {
            focus_and_select(&shared, &tab, backend_node).await?;
            send_key(&shared, &tab, &key).await?;
        }
        Action::Select { value } => {
            let object = resolve_object(&shared, &tab, backend_node).await?;
            call_function_on(
                &shared,
                &tab,
                &object,
                r#"function(value) {
                    if (!(this instanceof HTMLSelectElement)) {
                        throw new Error('referenced element is not a select');
                    }
                    const options = Array.from(this.options);
                    const option = options.find((item) => item.value === value)
                        || options.find((item) => item.label === value)
                        || options.find((item) => item.text === value);
                    if (!option) {
                        throw new Error('select has no option matching ' + JSON.stringify(value));
                    }
                    this.value = option.value;
                    this.dispatchEvent(new Event('input', { bubbles: true }));
                    this.dispatchEvent(new Event('change', { bubbles: true }));
                }"#,
                vec![json!({ "value": value })],
            )
            .await?;
        }
    }

    settle_after_action(&shared, browser_id, since).await;
    let page = refresh_page(&shared, &tab).await?;
    Ok(Reply::Acted { page })
}

async fn screenshot(
    shared: SharedState,
    tab: String,
    full_page: bool,
) -> Result<Reply, String> {
    let mut params = json!({ "format": "png" });
    if full_page {
        let metrics = cdp(&shared, &tab, "Page.getLayoutMetrics", json!({})).await?;
        let size = metrics
            .get("cssContentSize")
            .or_else(|| metrics.get("contentSize"))
            .ok_or_else(|| "Page.getLayoutMetrics returned no content size".to_owned())?;
        let width = size.get("width").and_then(Value::as_f64).unwrap_or(1280.0).ceil();
        let height = size.get("height").and_then(Value::as_f64).unwrap_or(800.0).ceil();
        params = json!({
            "format": "png",
            "captureBeyondViewport": true,
            "clip": {
                "x": 0,
                "y": 0,
                "width": width,
                "height": height,
                "scale": 1,
            },
        });
    }
    let result = cdp(&shared, &tab, "Page.captureScreenshot", params).await?;
    let data = result
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| "Page.captureScreenshot returned no data".to_owned())?;
    let png = STANDARD
        .decode(data)
        .map_err(|error| format!("Page.captureScreenshot returned invalid base64: {error}"))?;
    Ok(Reply::Screenshot { png })
}

async fn tabs(shared: SharedState, owner: String) -> Result<Reply, String> {
    ui_call(move || {
        let state = shared.lock().expect("browser state poisoned");
        let mut tabs = state
            .tabs
            .iter()
            .filter_map(|(id, tab)| {
                (tab.owner == owner).then_some(TabInfo {
                    tab: id.clone(),
                    profile: tab.profile.clone(),
                    page: tab.page.clone(),
                })
            })
            .collect::<Vec<_>>();
        tabs.sort_by(|left, right| left.tab.cmp(&right.tab));
        Reply::Tabs { tabs }
    })
    .await
}

async fn close(shared: SharedState, tab: String) -> Result<Reply, String> {
    let found = ui_call(move || {
        let taken = take_tab(&shared, &tab);
        if let Some(taken) = taken {
            close_browser(taken);
            true
        } else {
            false
        }
    })
    .await?;
    if found {
        Ok(Reply::Closed)
    } else {
        Err("unknown tab".to_owned())
    }
}

async fn close_owner(shared: SharedState, owner: String) -> Result<Reply, String> {
    ui_call(move || {
        let tabs = take_tabs_matching(&shared, |tab| tab.owner == owner);
        for tab in tabs {
            close_browser(tab);
        }
        Reply::Closed
    })
    .await
}

async fn delete_profile(shared: SharedState, profile: String) -> Result<Reply, String> {
    if !valid_profile_name(&profile) {
        return Err(format!("invalid profile name '{profile}'"));
    }
    let profiles_dir = {
        let shared_for_call = shared.clone();
        ui_call(move || {
            shared_for_call
                .lock()
                .expect("browser state poisoned")
                .profiles_dir
                .clone()
        })
        .await?
    };
    let profile_for_close = profile.clone();
    ui_call(move || {
        let (tabs, _context) = take_profile(&shared, &profile_for_close);
        for tab in tabs {
            close_browser(tab);
        }
    })
    .await?;

    // CEF finishes some profile I/O asynchronously after the last browser in a
    // request context closes. Retry briefly so DeleteProfile is observably
    // idempotent to the caller instead of racing that flush.
    let dir = profiles_dir.join(&profile);
    let mut last_error = None;
    for attempt in 0..10 {
        match fs::remove_dir_all(&dir) {
            Ok(()) => return Ok(Reply::Closed),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Reply::Closed);
            }
            Err(error) => {
                last_error = Some(error);
                if attempt < 9 {
                    sleep(Duration::from_millis(200)).await;
                }
            }
        }
    }
    Err(format!(
        "cannot delete profile directory '{}': {}",
        dir.display(),
        last_error.expect("delete loop records an error")
    ))
}
