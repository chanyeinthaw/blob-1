//! `Environment` implementation that proxies calls to a remote `blob-1 node`
//! over the `/ws/node` wire protocol. The primary's node session registry
//! implements `NodeDirectory`; a bot bound to a remote environment gets a
//! `RemoteEnvironment` scoped to its env and workspace names.

use std::collections::BTreeMap;
use std::time::Duration;

use environment::node::{NodeReply, NodeRequest};
use environment::{Environment, SkillBundle, SkillEntry, ToolCall, ToolError, ToolResult};
use tokio::sync::watch;

/// Bash can run long; skill calls are quick metadata/file touches.
const EXECUTE_TIMEOUT: Duration = Duration::from_secs(120);
const SKILLS_TIMEOUT: Duration = Duration::from_secs(30);

/// Routes environment requests to the node session live for each env.
#[async_trait::async_trait]
pub trait NodeDirectory: Send + Sync {
    /// Round-trips one environment request to the env's live node session.
    /// `enrollment` pins the accepted enrollment the caller bound — a route
    /// that moved to a re-enrolled node under the same name must fail.
    async fn call(
        &self,
        env: &str,
        request: NodeRequest,
        enrollment: Option<&[u8]>,
    ) -> Result<NodeReply, ToolError>;
    /// Watch the enrollment ID routed for `env` (`None` = unrouted).
    /// Identity, not just liveness, so consumers catch re-enrollment under
    /// the same name.
    fn availability(&self, env: &str) -> watch::Receiver<Option<Vec<u8>>>;
}

#[async_trait::async_trait]
impl<T: NodeDirectory + ?Sized> NodeDirectory for std::sync::Arc<T> {
    async fn call(
        &self,
        env: &str,
        request: NodeRequest,
        enrollment: Option<&[u8]>,
    ) -> Result<NodeReply, ToolError> {
        (**self).call(env, request, enrollment).await
    }

    fn availability(&self, env: &str) -> watch::Receiver<Option<Vec<u8>>> {
        (**self).availability(env)
    }
}

/// `Environment` that forwards each method to a remote node as a `NodeRequest`.
/// Calls and liveness are pinned to `enrollment`: when the env name is
/// re-enrolled (even to the same node), this instance reports offline
/// rather than following the name across.
pub struct RemoteEnvironment<D> {
    env: String,
    workspace: String,
    enrollment: Option<Vec<u8>>,
    online: watch::Receiver<bool>,
    directory: D,
    execute_timeout: Duration,
    skills_timeout: Duration,
}

impl<D: NodeDirectory> RemoteEnvironment<D> {
    /// `enrollment` pins the accepted enrollment the caller bound (from the
    /// storage snapshot that also produced `env`); `None` = never online.
    pub fn new(
        env: impl Into<String>,
        workspace: impl Into<String>,
        directory: D,
        enrollment: Option<Vec<u8>>,
    ) -> Self {
        let env = env.into();
        // Derive a bool "my enrollment is routed" watch from the identity
        // watch. A revoke+re-accept between polls reads as offline because
        // the enrollment differs, not as a coalesced true.
        let mut source = directory.availability(&env);
        let live = enrollment.is_some() && source.borrow().as_deref() == enrollment.as_deref();
        let (tx, rx) = watch::channel(live);
        tokio::spawn({
            let enrollment = enrollment.clone();
            async move {
                loop {
                    let live =
                        enrollment.is_some() && source.borrow().as_deref() == enrollment.as_deref();
                    tx.send_replace(live);
                    tokio::select! {
                        changed = source.changed() => {
                            if changed.is_err() {
                                break;
                            }
                        }
                        _ = tx.closed() => break,
                    }
                }
            }
        });
        Self {
            env,
            workspace: workspace.into(),
            enrollment,
            online: rx,
            directory,
            execute_timeout: EXECUTE_TIMEOUT,
            skills_timeout: SKILLS_TIMEOUT,
        }
    }

    /// True while the env's routed session is the enrollment this instance
    /// pinned at construction; `changed().await` fires on any transition.
    pub fn availability(&self) -> watch::Receiver<bool> {
        self.online.clone()
    }

    async fn call(&self, request: NodeRequest, timeout: Duration) -> Result<NodeReply, ToolError> {
        let call = self
            .directory
            .call(&self.env, request, self.enrollment.as_deref());
        match tokio::time::timeout(timeout, call).await {
            Ok(reply) => reply,
            Err(_) => Err(ToolError::Io("environment request timed out".into())),
        }
    }
}

/// A node-side failure or protocol violation, surfaced as an I/O error.
fn unexpected(reply: NodeReply) -> ToolError {
    match reply {
        NodeReply::Error { error } => ToolError::Io(error),
        _ => ToolError::Io("unexpected reply from node".into()),
    }
}

#[async_trait::async_trait]
impl<D: NodeDirectory> Environment for RemoteEnvironment<D> {
    async fn managed_skills(&self) -> Result<BTreeMap<String, (String, bool)>, ToolError> {
        let request = NodeRequest::ManagedSkills {
            workspace: self.workspace.clone(),
        };
        match self.call(request, self.skills_timeout).await? {
            NodeReply::ManagedSkills { manifest } => Ok(manifest),
            other => Err(unexpected(other)),
        }
    }

    async fn apply_skills(
        &self,
        put: Vec<SkillBundle>,
        remove: Vec<String>,
    ) -> Result<Vec<String>, ToolError> {
        let request = NodeRequest::ApplySkills {
            workspace: self.workspace.clone(),
            put,
            remove,
        };
        match self.call(request, self.skills_timeout).await? {
            NodeReply::ApplySkills { skipped } => Ok(skipped),
            other => Err(unexpected(other)),
        }
    }

    async fn skill_catalog(&self) -> Result<Vec<SkillEntry>, ToolError> {
        let request = NodeRequest::SkillCatalog {
            workspace: self.workspace.clone(),
        };
        match self.call(request, self.skills_timeout).await? {
            NodeReply::SkillCatalog { entries } => Ok(entries),
            other => Err(unexpected(other)),
        }
    }

    async fn execute(&self, call: ToolCall) -> Result<ToolResult, ToolError> {
        let request = NodeRequest::Execute {
            workspace: self.workspace.clone(),
            call,
        };
        match self.call(request, self.execute_timeout).await? {
            NodeReply::Execute { result } => Ok(result),
            other => Err(unexpected(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use environment::{Image, ToolInput};
    use std::collections::BTreeMap;
    use tokio::sync::{mpsc, oneshot};

    type Queued = (String, NodeRequest, oneshot::Sender<NodeReply>);

    /// Captures each request on a channel; the test answers it (or not).
    struct StubDirectory {
        requests: mpsc::UnboundedSender<Queued>,
        live: watch::Sender<Option<Vec<u8>>>,
    }

    fn pin() -> Option<Vec<u8>> {
        Some(b"stub-cred".to_vec())
    }

    impl StubDirectory {
        fn new() -> (Self, mpsc::UnboundedReceiver<Queued>) {
            let (requests, rx) = mpsc::unbounded_channel();
            let (live, _) = watch::channel(pin());
            (Self { requests, live }, rx)
        }
    }

    #[async_trait::async_trait]
    impl NodeDirectory for StubDirectory {
        async fn call(
            &self,
            env: &str,
            request: NodeRequest,
            _enrollment: Option<&[u8]>,
        ) -> Result<NodeReply, ToolError> {
            let (tx, rx) = oneshot::channel();
            self.requests.send((env.to_owned(), request, tx)).unwrap();
            rx.await
                .map_err(|_| ToolError::Io("environment offline".into()))
        }

        fn availability(&self, _env: &str) -> watch::Receiver<Option<Vec<u8>>> {
            self.live.subscribe()
        }
    }

    fn bash_call() -> ToolCall {
        ToolCall {
            id: "t1".into(),
            input: ToolInput::Bash {
                command: "ls".into(),
                timeout: None,
            },
        }
    }

    #[tokio::test]
    async fn execute_roundtrips_result() {
        let (stub, mut rx) = StubDirectory::new();
        let env = RemoteEnvironment::new("gpu", "bot-1", stub, pin());
        let call = bash_call();
        let expected = ToolResult {
            id: call.id.clone(),
            content: "ok".into(),
            images: vec![Image {
                mime: "image/png".into(),
                data: vec![1, 2, 3],
            }],
        };
        let want = expected.clone();
        let task = tokio::spawn(async move { env.execute(call).await });

        let (env_name, request, reply) = rx.recv().await.unwrap();
        assert_eq!(env_name, "gpu");
        let NodeRequest::Execute {
            workspace,
            call: request_call,
        } = request
        else {
            panic!("expected execute request");
        };
        assert_eq!(workspace, "bot-1");
        assert_eq!(request_call.id, expected.id);
        reply.send(NodeReply::Execute { result: want }).unwrap();

        assert_eq!(task.await.unwrap().unwrap(), expected);
    }

    #[tokio::test]
    async fn node_error_maps_to_io() {
        let (stub, mut rx) = StubDirectory::new();
        let env = RemoteEnvironment::new("gpu", "bot-1", stub, pin());
        let task = tokio::spawn(async move { env.skill_catalog().await });

        let (_, _, reply) = rx.recv().await.unwrap();
        reply
            .send(NodeReply::Error {
                error: "no such workspace".into(),
            })
            .unwrap();

        match task.await.unwrap() {
            Err(ToolError::Io(error)) => assert_eq!(error, "no such workspace"),
            other => panic!("expected io error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unanswered_request_times_out() {
        let (stub, _rx) = StubDirectory::new();
        let mut env = RemoteEnvironment::new("gpu", "bot-1", stub, pin());
        env.execute_timeout = Duration::from_millis(10);

        match env.execute(bash_call()).await {
            Err(ToolError::Io(error)) => assert_eq!(error, "environment request timed out"),
            other => panic!("expected timeout, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn skill_methods_map_replies() {
        let (stub, mut rx) = StubDirectory::new();
        let env = RemoteEnvironment::new("gpu", "bot-1", stub, pin());
        let task = tokio::spawn(async move { env.managed_skills().await });

        let (env_name, request, reply) = rx.recv().await.unwrap();
        assert_eq!(env_name, "gpu");
        let NodeRequest::ManagedSkills { workspace } = request else {
            panic!("expected managed-skills request");
        };
        assert_eq!(workspace, "bot-1");
        let manifest = BTreeMap::from([("git".to_owned(), ("abc".to_owned(), true))]);
        reply
            .send(NodeReply::ManagedSkills {
                manifest: manifest.clone(),
            })
            .unwrap();

        assert_eq!(task.await.unwrap().unwrap(), manifest);
    }

    #[tokio::test]
    async fn availability_passthrough() {
        let (stub, _rx) = StubDirectory::new();
        let env = RemoteEnvironment::new("gpu", "bot-1", stub, pin());
        let receiver = env.availability();
        assert!(*receiver.borrow());
    }
}
