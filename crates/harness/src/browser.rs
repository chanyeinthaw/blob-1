use std::borrow::Cow;
use std::sync::Arc;

use browser_protocol::{Action, Reply, Request};
use environment::{Image, ToolSpec};
use serde::Deserialize;

/// Server-side browser access the harness exposes to task sessions. The
/// implementation lives in `agent`, which owns the companion process and the
/// tab ownership map.
#[async_trait::async_trait]
pub trait BrowserBackend: Send + Sync {
    /// Whether the companion is connected and ready right now.
    fn ready(&self) -> bool;
    /// The browser profile currently assigned to this bot.
    fn profile(&self) -> Result<Option<String>, String>;
    /// Resolves one of this bot's secrets. The value is only ever sent to the
    /// companion; tool results carry the secret's name, never the value.
    fn secret(&self, name: &str) -> Result<Option<String>, String>;
    /// Runs one companion request for `owner` (the task session id).
    async fn execute(&self, owner: &str, request: Request) -> Result<Reply, String>;
}

/// Text-plus-image result of a browser tool call.
pub struct BrowserOutcome {
    pub content: String,
    pub images: Vec<Image>,
}

impl BrowserOutcome {
    fn text(content: String) -> Self {
        Self {
            content,
            images: Vec::new(),
        }
    }
}

/// Dynamic task browser tools. `specs` is empty while the bot has no profile
/// or the companion is not ready, so tasks only see the tools when usable.
#[derive(Clone)]
pub struct BrowserTools {
    backend: Arc<dyn BrowserBackend>,
}

impl BrowserTools {
    pub fn new(backend: Arc<dyn BrowserBackend>) -> Self {
        Self { backend }
    }

    pub fn enabled(&self) -> bool {
        self.backend.ready() && self.backend.profile().ok().flatten().is_some()
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        if self.enabled() {
            TOOLS.to_vec()
        } else {
            Vec::new()
        }
    }

    /// Closes every tab owned by a finished task. Cleanup is best-effort: the
    /// companion exiting also closes its tabs, and a dead companion must not
    /// block task completion.
    pub async fn close_owner(&self, owner: &str) {
        let _ = self
            .backend
            .execute(owner, Request::CloseOwner {
                owner: owner.to_owned(),
            })
            .await;
    }

    pub async fn execute(
        &self,
        owner: &str,
        name: &str,
        arguments: &str,
    ) -> Result<BrowserOutcome, String> {
        if !self.backend.ready() {
            return Err("browser is not ready".into());
        }
        let profile = self
            .backend
            .profile()?
            .ok_or_else(|| "this bot has no browser profile".to_owned())?;
        match name {
            "browser_open" => {
                let args: OpenArgs = parse(arguments)?;
                let reply = self
                    .backend
                    .execute(
                        owner,
                        Request::Open {
                            profile,
                            owner: owner.to_owned(),
                            url: args.url,
                        },
                    )
                    .await?;
                let Reply::Opened { tab, page } = reply else {
                    return Err(unexpected("browser_open"));
                };
                Ok(BrowserOutcome::text(format!(
                    "tab: {tab}\n{}",
                    page_text(&page)
                )))
            }
            "browser_navigate" => {
                let args: NavigateArgs = parse(arguments)?;
                let reply = self
                    .backend
                    .execute(
                        owner,
                        Request::Navigate {
                            tab: args.tab,
                            url: args.url,
                        },
                    )
                    .await?;
                let Reply::Navigated { page } = reply else {
                    return Err(unexpected("browser_navigate"));
                };
                Ok(BrowserOutcome::text(page_text(&page)))
            }
            "browser_snapshot" => {
                let args: TabArgs = parse(arguments)?;
                let reply = self
                    .backend
                    .execute(owner, Request::Snapshot { tab: args.tab })
                    .await?;
                let Reply::Snapshot { page, tree } = reply else {
                    return Err(unexpected("browser_snapshot"));
                };
                Ok(BrowserOutcome::text(format!("{}\n\n{tree}", page_text(&page))))
            }
            "browser_act" => {
                let args: ActArgs = parse(arguments)?;
                let mut secret_name = None;
                let action = match args.action.as_str() {
                    "click" => Action::Click,
                    "type" => {
                        let text = match (args.text, args.secret) {
                            (Some(text), None) => text,
                            (None, Some(name)) => {
                                let name = name.trim().to_owned();
                                if !storage::valid_secret_name(&name) {
                                    return Err(format!("invalid secret name '{name}'"));
                                }
                                let value = self
                                    .backend
                                    .secret(&name)?
                                    .ok_or_else(|| format!("no secret named '{name}'"))?;
                                secret_name = Some(name);
                                value
                            }
                            _ => {
                                return Err(
                                    "type requires exactly one of 'text' or 'secret'".into()
                                )
                            }
                        };
                        Action::Type {
                            text,
                            submit: args.submit.unwrap_or(false),
                        }
                    }
                    "press" => Action::Press {
                        key: args.key.ok_or("press requires 'key'")?,
                    },
                    "select" => Action::Select {
                        value: args.value.ok_or("select requires 'value'")?,
                    },
                    "hover" => Action::Hover,
                    action => return Err(format!("unknown browser action '{action}'")),
                };
                let reply = self
                    .backend
                    .execute(
                        owner,
                        Request::Act {
                            tab: args.tab,
                            element: args.element,
                            action,
                        },
                    )
                    .await?;
                let Reply::Acted { page } = reply else {
                    return Err(unexpected("browser_act"));
                };
                let action = match secret_name {
                    Some(name) => format!("typed secret {name}"),
                    None => format!("action: {}", args.action),
                };
                Ok(BrowserOutcome::text(format!(
                    "{action}\n{}",
                    page_text(&page)
                )))
            }
            "browser_evaluate" => {
                let args: EvaluateArgs = parse(arguments)?;
                let reply = self
                    .backend
                    .execute(
                        owner,
                        Request::Evaluate {
                            tab: args.tab,
                            expression: args.expression,
                        },
                    )
                    .await?;
                let Reply::Evaluated { value } = reply else {
                    return Err(unexpected("browser_evaluate"));
                };
                Ok(BrowserOutcome::text(
                    serde_json::to_string(&value).map_err(|error| error.to_string())?,
                ))
            }
            "browser_screenshot" => {
                let args: ScreenshotArgs = parse(arguments)?;
                let reply = self
                    .backend
                    .execute(
                        owner,
                        Request::Screenshot {
                            tab: args.tab,
                            full_page: args.full_page.unwrap_or(false),
                        },
                    )
                    .await?;
                let Reply::Screenshot { png } = reply else {
                    return Err(unexpected("browser_screenshot"));
                };
                Ok(BrowserOutcome {
                    content: "Screenshot [image/png]".into(),
                    images: vec![Image {
                        mime: "image/png".into(),
                        data: png,
                    }],
                })
            }
            "browser_tabs" => {
                let reply = self
                    .backend
                    .execute(
                        owner,
                        Request::Tabs {
                            owner: owner.to_owned(),
                        },
                    )
                    .await?;
                let Reply::Tabs { tabs } = reply else {
                    return Err(unexpected("browser_tabs"));
                };
                if tabs.is_empty() {
                    return Ok(BrowserOutcome::text("No open tabs.".into()));
                }
                Ok(BrowserOutcome::text(
                    tabs.into_iter()
                        .map(|tab| format!("tab: {}\n{}", tab.tab, page_text(&tab.page)))
                        .collect::<Vec<_>>()
                        .join("\n\n"),
                ))
            }
            "browser_close" => {
                let args: TabArgs = parse(arguments)?;
                let reply = self
                    .backend
                    .execute(owner, Request::Close { tab: args.tab })
                    .await?;
                let Reply::Closed = reply else {
                    return Err(unexpected("browser_close"));
                };
                Ok(BrowserOutcome::text("closed".into()))
            }
            _ => Err(format!("unknown tool: {name}")),
        }
    }
}

fn parse<'a, T: Deserialize<'a>>(arguments: &'a str) -> Result<T, String> {
    serde_json::from_str(arguments).map_err(|error| error.to_string())
}

fn unexpected(tool: &str) -> String {
    format!("browser companion returned an unexpected reply for {tool}")
}

fn page_text(page: &browser_protocol::Page) -> String {
    format!("url: {}\ntitle: {}", page.url, page.title)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenArgs {
    url: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NavigateArgs {
    tab: String,
    url: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TabArgs {
    tab: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActArgs {
    tab: String,
    #[serde(rename = "ref")]
    element: String,
    action: String,
    text: Option<String>,
    secret: Option<String>,
    submit: Option<bool>,
    key: Option<String>,
    value: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvaluateArgs {
    tab: String,
    expression: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScreenshotArgs {
    tab: String,
    #[serde(rename = "fullPage")]
    full_page: Option<bool>,
}

const TOOLS: &[ToolSpec] = &[
    ToolSpec {
        name: Cow::Borrowed("browser_open"),
        description: Cow::Borrowed("Open a browser tab in this bot's persistent profile, optionally navigating to url. Returns the tab id, url and title."),
        input_schema: Cow::Borrowed(r#"{"type":"object","properties":{"url":{"type":"string","description":"URL to load; omit for a blank tab"}}}"#),
    },
    ToolSpec {
        name: Cow::Borrowed("browser_navigate"),
        description: Cow::Borrowed("Navigate one of this task's browser tabs to a URL and wait for the load."),
        input_schema: Cow::Borrowed(r#"{"type":"object","properties":{"tab":{"type":"string"},"url":{"type":"string"}},"required":["tab","url"]}"#),
    },
    ToolSpec {
        name: Cow::Borrowed("browser_snapshot"),
        description: Cow::Borrowed("Read a browser tab as an indented text tree. Actionable elements carry refs for browser_act; take a fresh snapshot after navigation or page changes."),
        input_schema: Cow::Borrowed(r#"{"type":"object","properties":{"tab":{"type":"string"}},"required":["tab"]}"#),
    },
    ToolSpec {
        name: Cow::Borrowed("browser_act"),
        description: Cow::Borrowed("Act on a snapshot ref: click, type, press a key, select an option, or hover. For type pass exactly one of text or secret (the name of one of this bot's secrets); secret values are typed without entering the conversation."),
        input_schema: Cow::Borrowed(r#"{"type":"object","properties":{"tab":{"type":"string"},"ref":{"type":"string"},"action":{"type":"string","enum":["click","type","press","select","hover"]},"text":{"type":"string","description":"Text to type"},"secret":{"type":"string","description":"Name of a stored secret to type"},"submit":{"type":"boolean","description":"Press Enter after typing"},"key":{"type":"string","description":"Key for press, such as Enter, Tab, Escape, ArrowDown"},"value":{"type":"string","description":"Option value or label for select"}},"required":["tab","ref","action"]}"#),
    },
    ToolSpec {
        name: Cow::Borrowed("browser_evaluate"),
        description: Cow::Borrowed("Evaluate a JavaScript expression in a browser tab's main frame and return its JSON value."),
        input_schema: Cow::Borrowed(r#"{"type":"object","properties":{"tab":{"type":"string"},"expression":{"type":"string"}},"required":["tab","expression"]}"#),
    },
    ToolSpec {
        name: Cow::Borrowed("browser_screenshot"),
        description: Cow::Borrowed("Capture a browser tab as a PNG image. Set fullPage to capture beyond the viewport."),
        input_schema: Cow::Borrowed(r#"{"type":"object","properties":{"tab":{"type":"string"},"fullPage":{"type":"boolean"}},"required":["tab"]}"#),
    },
    ToolSpec {
        name: Cow::Borrowed("browser_tabs"),
        description: Cow::Borrowed("List this task's open browser tabs."),
        input_schema: Cow::Borrowed(r#"{"type":"object","properties":{}}"#),
    },
    ToolSpec {
        name: Cow::Borrowed("browser_close"),
        description: Cow::Borrowed("Close one of this task's browser tabs."),
        input_schema: Cow::Borrowed(r#"{"type":"object","properties":{"tab":{"type":"string"}},"required":["tab"]}"#),
    },
];
