use browser_protocol::{valid_profile_name, Page};
use cef::*;
use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    fs,
    os::raw::c_int,
    path::PathBuf,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::oneshot;

pub type SharedState = Arc<Mutex<BrowserState>>;
pub type CdpOutcome = Result<Value, String>;

pub struct PendingCdp {
    pub tab: String,
    pub sender: oneshot::Sender<CdpOutcome>,
}

pub struct PendingCreation {
    pub tab: String,
    pub owner: String,
    pub profile: String,
    pub sender: oneshot::Sender<Result<i32, String>>,
}

pub struct TabState {
    pub owner: String,
    pub profile: String,
    pub browser: Browser,
    pub browser_id: i32,
    pub _registration: Option<Registration>,
    pub refs: HashMap<String, i64>,
    pub page: Page,
}

pub struct BrowserState {
    pub profiles_dir: PathBuf,
    pub profiles: HashMap<String, RequestContext>,
    pub tabs: HashMap<String, TabState>,
    pub next_tab: u64,
    pub pending_cdp: HashMap<i32, PendingCdp>,
    pub creation_queue: VecDeque<PendingCreation>,
    pub load_seq: u64,
    pub load_ends: HashMap<i32, u64>,
    pub load_errors: HashMap<i32, (u64, String)>,
}

impl BrowserState {
    pub fn new(profiles_dir: PathBuf) -> Self {
        Self {
            profiles_dir,
            profiles: HashMap::new(),
            tabs: HashMap::new(),
            next_tab: 0,
            pending_cdp: HashMap::new(),
            creation_queue: VecDeque::new(),
            load_seq: 0,
            load_ends: HashMap::new(),
            load_errors: HashMap::new(),
        }
    }

    pub fn note_load_end(&mut self, browser_id: i32) {
        self.load_seq += 1;
        self.load_ends.insert(browser_id, self.load_seq);
    }

    pub fn note_load_error(&mut self, browser_id: i32, error: String) {
        self.load_seq += 1;
        self.load_errors.insert(browser_id, (self.load_seq, error));
    }

    pub fn note_title(&mut self, browser_id: i32, title: String) {
        for tab in self.tabs.values_mut() {
            if tab.browser_id == browser_id {
                tab.page.title = title;
                return;
            }
        }
    }

    pub fn fail_pending_for_tab(&mut self, tab: &str, reason: &str) {
        let ids = self
            .pending_cdp
            .iter()
            .filter_map(|(id, pending)| (pending.tab == tab).then_some(*id))
            .collect::<Vec<_>>();
        for id in ids {
            if let Some(pending) = self.pending_cdp.remove(&id) {
                let _ = pending.sender.send(Err(reason.to_owned()));
            }
        }
    }

    pub fn remove_browser_tab(&mut self, browser_id: i32) {
        let tab = self
            .tabs
            .iter()
            .find_map(|(id, tab)| (tab.browser_id == browser_id).then_some(id.clone()));
        if let Some(tab) = tab {
            self.tabs.remove(&tab);
            self.fail_pending_for_tab(&tab, "tab closed");
        }
    }
}

fn cef_path(path: &std::path::Path) -> CefString {
    CefString::from(path.to_string_lossy().as_ref())
}

fn create_profile_context(
    state: &mut BrowserState,
    profile: &str,
) -> Result<RequestContext, String> {
    if let Some(context) = state.profiles.get(profile) {
        return Ok(context.clone());
    }

    let dir = state.profiles_dir.join(profile);
    fs::create_dir_all(&dir).map_err(|error| format!("cannot create profile directory: {error}"))?;
    let settings = RequestContextSettings {
        cache_path: cef_path(&dir),
        persist_session_cookies: 1,
        ..Default::default()
    };
    let context = request_context_create_context(Some(&settings), None)
        .ok_or_else(|| format!("cannot create browser profile '{profile}'"))?;
    state.profiles.insert(profile.to_owned(), context.clone());
    Ok(context)
}

/// Starts creation of a windowless browser. This must run on CEF's UI thread;
/// the state lock is deliberately dropped around browser creation because CEF
/// can invoke client callbacks re-entrantly.
pub fn create_tab_on_ui(
    shared: &SharedState,
    profile: &str,
    owner: &str,
) -> Result<(String, oneshot::Receiver<Result<i32, String>>), String> {
    if !valid_profile_name(profile) {
        return Err(format!("invalid profile name '{profile}'"));
    }

    let (tab_id, mut context, receiver) = {
        let mut state = shared.lock().expect("browser state poisoned");
        state.next_tab += 1;
        let tab_id = format!("t{}", state.next_tab);
        let context = create_profile_context(&mut state, profile)?;
        let (sender, receiver) = oneshot::channel();
        state.creation_queue.push_back(PendingCreation {
            tab: tab_id.clone(),
            owner: owner.to_owned(),
            profile: profile.to_owned(),
            sender,
        });
        (tab_id, context, receiver)
    };

    let weak = Arc::downgrade(shared);
    let mut client = HeadlessClient::new(weak);
    let window_info = WindowInfo {
        windowless_rendering_enabled: 1,
        runtime_style: RuntimeStyle::ALLOY,
        ..Default::default()
    };
    let browser_settings = BrowserSettings {
        windowless_frame_rate: 30,
        ..Default::default()
    };
    let url = CefString::from("about:blank");
    let accepted = browser_host_create_browser(
        Some(&window_info),
        Some(&mut client),
        Some(&url),
        Some(&browser_settings),
        None,
        Some(&mut context),
    );
    if accepted == 0 {
        cancel_creation_on_ui(shared, &tab_id);
        return Err("CEF rejected browser creation".to_owned());
    }
    Ok((tab_id, receiver))
}

/// Cancels a browser creation that has not reported OnAfterCreated yet.
/// Returns false when creation has already completed or never existed.
pub fn cancel_creation_on_ui(shared: &SharedState, tab: &str) -> bool {
    let mut state = shared.lock().expect("browser state poisoned");
    let Some(position) = state
        .creation_queue
        .iter()
        .position(|pending| pending.tab == tab)
    else {
        return false;
    };
    state.creation_queue.remove(position);
    true
}

pub fn take_tab(shared: &SharedState, tab: &str) -> Option<TabState> {
    let mut state = shared.lock().expect("browser state poisoned");
    let taken = state.tabs.remove(tab)?;
    state.fail_pending_for_tab(tab, "tab closed");
    Some(taken)
}

pub fn close_browser(tab: TabState) {
    if let Some(host) = tab.browser.host() {
        host.close_browser(1);
    }
}

pub fn take_tabs_matching(
    shared: &SharedState,
    predicate: impl Fn(&TabState) -> bool,
) -> Vec<TabState> {
    let mut state = shared.lock().expect("browser state poisoned");
    let ids = state
        .tabs
        .iter()
        .filter_map(|(id, tab)| predicate(tab).then_some(id.clone()))
        .collect::<Vec<_>>();
    let mut taken = Vec::new();
    for id in ids {
        if let Some(tab) = state.tabs.remove(&id) {
            state.fail_pending_for_tab(&id, "tab closed");
            taken.push(tab);
        }
    }
    taken
}

pub fn take_profile(
    shared: &SharedState,
    profile: &str,
) -> (Vec<TabState>, Option<RequestContext>) {
    let tabs = take_tabs_matching(shared, |tab| tab.profile == profile);
    let context = shared
        .lock()
        .expect("browser state poisoned")
        .profiles
        .remove(profile);
    (tabs, context)
}

pub fn close_all_browsers(shared: &SharedState) {
    let tabs = take_tabs_matching(shared, |_| true);
    for tab in tabs {
        close_browser(tab);
    }
}

wrap_app! {
    pub struct HeadlessApp;

    impl App {
        fn on_before_command_line_processing(
            &self,
            _process_type: Option<&CefStringUtf16>,
            command_line: Option<&mut CommandLine>,
        ) {
            let Some(command_line) = command_line else {
                return;
            };

            let ozone = CefString::from("ozone-platform");
            let headless = CefString::from("headless");
            command_line.append_switch_with_value(Some(&ozone), Some(&headless));
            let password_store = CefString::from("password-store");
            let basic = CefString::from("basic");
            command_line.append_switch_with_value(Some(&password_store), Some(&basic));
            for switch in [
                "disable-gpu",
                "disable-gpu-compositing",
                "no-first-run",
                "no-default-browser-check",
                "use-mock-keychain",
                "disable-background-networking",
                "disable-component-update",
            ] {
                let switch = CefString::from(switch);
                command_line.append_switch(Some(&switch));
            }
        }
    }
}

wrap_client! {
    struct HeadlessClient {
        state: Weak<Mutex<BrowserState>>,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(HeadlessRenderHandler::new())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(HeadlessLifeSpanHandler::new(self.state.clone()))
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(HeadlessLoadHandler::new(self.state.clone()))
        }

        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(HeadlessDisplayHandler::new(self.state.clone()))
        }
    }
}

wrap_render_handler! {
    struct HeadlessRenderHandler;

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect {
                rect.x = 0;
                rect.y = 0;
                rect.width = 1280;
                rect.height = 800;
            }
        }

        fn screen_info(
            &self,
            _browser: Option<&mut Browser>,
            screen_info: Option<&mut ScreenInfo>,
        ) -> c_int {
            if let Some(screen_info) = screen_info {
                screen_info.device_scale_factor = 1.0;
                return 1;
            }
            0
        }
    }
}

wrap_life_span_handler! {
    struct HeadlessLifeSpanHandler {
        state: Weak<Mutex<BrowserState>>,
    }

    impl LifeSpanHandler {
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: c_int,
            _target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            _user_gesture: c_int,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut c_int>,
        ) -> c_int {
            // Popup browsers are not addressable through the companion protocol.
            1
        }

        fn on_after_created(&self, browser: Option<&mut Browser>) {
            let (Some(browser), Some(shared)) = (browser, self.state.upgrade()) else {
                return;
            };
            let pending = shared
                .lock()
                .expect("browser state poisoned")
                .creation_queue
                .pop_front();
            let Some(pending) = pending else {
                if let Some(host) = browser.host() {
                    host.close_browser(1);
                }
                return;
            };

            let browser_id = browser.identifier();
            let weak = Arc::downgrade(&shared);
            let registration = browser.host().and_then(|host| {
                let mut observer = HeadlessDevToolsObserver::new(weak);
                host.add_dev_tools_message_observer(Some(&mut observer))
            });
            shared
                .lock()
                .expect("browser state poisoned")
                .tabs
                .insert(
                    pending.tab.clone(),
                    TabState {
                        owner: pending.owner,
                        profile: pending.profile,
                        browser: browser.clone(),
                        browser_id,
                        _registration: registration,
                        refs: HashMap::new(),
                        page: Page {
                            url: "about:blank".to_owned(),
                            title: String::new(),
                        },
                    },
                );
            let _ = pending.sender.send(Ok(browser_id));
        }

        fn on_before_close(&self, browser: Option<&mut Browser>) {
            let (Some(browser), Some(shared)) = (browser, self.state.upgrade()) else {
                return;
            };
            shared
                .lock()
                .expect("browser state poisoned")
                .remove_browser_tab(browser.identifier());
        }
    }
}

wrap_load_handler! {
    struct HeadlessLoadHandler {
        state: Weak<Mutex<BrowserState>>,
    }

    impl LoadHandler {
        fn on_loading_state_change(
            &self,
            browser: Option<&mut Browser>,
            is_loading: c_int,
            _can_go_back: c_int,
            _can_go_forward: c_int,
        ) {
            if is_loading != 0 {
                return;
            }
            let (Some(browser), Some(shared)) = (browser, self.state.upgrade()) else {
                return;
            };
            shared
                .lock()
                .expect("browser state poisoned")
                .note_load_end(browser.identifier());
        }

        fn on_load_error(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            error_code: Errorcode,
            error_text: Option<&CefString>,
            failed_url: Option<&CefString>,
        ) {
            let (Some(browser), Some(frame), Some(shared)) =
                (browser, frame, self.state.upgrade())
            else {
                return;
            };
            if frame.is_main() == 0 {
                return;
            }
            let text = error_text.map(CefString::to_string).unwrap_or_default();
            let url = failed_url.map(CefString::to_string).unwrap_or_default();
            let detail = if text.is_empty() {
                format!("navigation failed for {url}: {error_code:?}")
            } else {
                format!("navigation failed for {url}: {text}")
            };
            shared
                .lock()
                .expect("browser state poisoned")
                .note_load_error(browser.identifier(), detail);
        }
    }
}

wrap_display_handler! {
    struct HeadlessDisplayHandler {
        state: Weak<Mutex<BrowserState>>,
    }

    impl DisplayHandler {
        fn on_title_change(&self, browser: Option<&mut Browser>, title: Option<&CefString>) {
            let (Some(browser), Some(shared)) = (browser, self.state.upgrade()) else {
                return;
            };
            let title = title.map(CefString::to_string).unwrap_or_default();
            shared
                .lock()
                .expect("browser state poisoned")
                .note_title(browser.identifier(), title);
        }
    }
}

wrap_dev_tools_message_observer! {
    struct HeadlessDevToolsObserver {
        state: Weak<Mutex<BrowserState>>,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(
            &self,
            _browser: Option<&mut Browser>,
            message_id: c_int,
            success: c_int,
            result: Option<&[u8]>,
        ) {
            let Some(shared) = self.state.upgrade() else {
                return;
            };
            let value = result
                .map(|bytes| {
                    serde_json::from_slice::<Value>(bytes).unwrap_or(Value::Null)
                })
                .unwrap_or(Value::Null);
            let outcome = if success != 0 {
                Ok(value)
            } else {
                let message = value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown DevTools protocol error");
                Err(format!("CDP error: {message}"))
            };
            let pending = shared
                .lock()
                .expect("browser state poisoned")
                .pending_cdp
                .remove(&message_id);
            if let Some(pending) = pending {
                let _ = pending.sender.send(outcome);
            }
        }
    }
}

type UiJob = Box<dyn FnOnce() + Send + 'static>;

wrap_task! {
    struct PostedUiTask {
        job: Arc<Mutex<Option<UiJob>>>,
    }

    impl Task {
        fn execute(&self) {
            let job = self.job.lock().expect("UI job poisoned").take();
            if let Some(job) = job {
                job();
            }
        }
    }
}

pub fn post_ui(job: impl FnOnce() + Send + 'static) -> Result<(), String> {
    let job = Arc::new(Mutex::new(Some(Box::new(job) as UiJob)));
    let mut task = PostedUiTask::new(job);
    if post_task(ThreadId::UI, Some(&mut task)) == 0 {
        Err("failed to post work to the CEF UI thread".to_owned())
    } else {
        Ok(())
    }
}
