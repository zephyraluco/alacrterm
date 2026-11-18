use super::*;
use crate::{
    provider::{CredentialSource, ModelProfile, Protocol},
    types::{EventKind, Mode},
};
use futures::FutureExt as _;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::broadcast,
};
use uuid::Uuid;

async fn fixture(responses: Vec<String>) -> (String, Arc<Mutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = requests.clone();
    tokio::spawn(async move {
        for response in responses {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let header_end = loop {
                let mut buf = [0; 4096];
                let n = stream.read(&mut buf).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
                if let Some(offset) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                    break offset + 4;
                }
                assert!(bytes.len() < 128 * 1024);
            };
            let header = String::from_utf8_lossy(&bytes[..header_end]).to_lowercase();
            assert!(header.starts_with("post /v1/chat/completions "));
            let len: usize = header
                .lines()
                .find_map(|s| s.strip_prefix("content-length:"))
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            while bytes.len() < header_end + len {
                let mut buf = [0; 4096];
                let n = stream.read(&mut buf).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
            }
            captured
                .lock()
                .unwrap()
                .push(serde_json::from_slice(&bytes[header_end..header_end + len]).unwrap());
            if response == "stall" {
                futures::future::pending::<()>().await;
            }
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response.len()
            );
            stream.write_all(headers.as_bytes()).await.unwrap();
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });
    (url, requests)
}

fn text_sse(text: &str) -> String {
    format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        json!({"id":"r1","object":"chat.completion.chunk","created":1,"model":"fixture","choices":[{"index":0,"delta":{"role":"assistant","content":text},"finish_reason":null}]}),
        json!({"id":"r1","object":"chat.completion.chunk","created":1,"model":"fixture","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}})
    )
}

fn request(session: Uuid, url: String, prompt: &str) -> StartRun {
    StartRun {
        session,
        model: ModelProfile {
            id: Uuid::new_v4(),
            name: "Fixture".into(),
            model: "fixture".into(),
            base_url: url,
            protocol: Protocol::OpenaiChat,
            metadata: None,
            credential: CredentialSource::None,
        },
        secret: None,
        prompt: prompt.into(),
        mode: Mode::Ask,
        target: None,
        reasoning: None,
        tools: vec![],
        instructions: String::new(),
    }
}

async fn ending(rx: &mut broadcast::Receiver<crate::types::Event>) -> EventKind {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let event = rx.recv().await.unwrap();
            if event.kind.is_terminal() {
                return event.kind;
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn native_history_survives_restart_and_observers_do_not_own_runs() {
    let (url, requests) = fixture(vec![text_sse("first answer"), text_sse("second answer")]).await;
    let directory = tempfile::tempdir().unwrap();
    let service = AgentService::open(directory.path()).unwrap();
    let session = service.create_session().unwrap().id;
    let mut receiver = service.subscribe();
    service
        .start(request(session, url.clone(), "hello"))
        .unwrap();
    assert!(matches!(ending(&mut receiver).await, EventKind::Completed));
    assert!(
        service
            .events(session, 0)
            .unwrap()
            .iter()
            .any(|e| matches!(&e.kind,EventKind::TextDelta{text} if text=="first answer"))
    );
    drop(service);
    let service = AgentService::open(directory.path()).unwrap();
    let receiver = service.subscribe();
    service.start(request(session, url, "remember me")).unwrap();
    drop(receiver);
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if service.sessions().unwrap()[0].active_run.is_none() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let history = requests[1]["messages"].to_string();
    assert!(history.contains("first answer"), "{history}");
    assert!(history.contains("hello"));
    assert!(history.contains("remember me"));
    assert!(history.contains("You are Alacrterm"));
    assert!(!history.contains("You are solaris"));
}

#[tokio::test]
async fn stalled_network_can_be_cancelled_and_a_new_run_started() {
    let (url, requests) = fixture(vec!["stall".into()]).await;
    let directory = tempfile::tempdir().unwrap();
    let service = AgentService::open(directory.path()).unwrap();
    let session = service.create_session().unwrap().id;
    let mut receiver = service.subscribe();
    service
        .start(request(session, url.clone(), "hello"))
        .unwrap();
    assert!(
        service
            .start(request(session, url.clone(), "overlap"))
            .is_err()
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while requests.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    service.cancel(session);
    assert!(matches!(ending(&mut receiver).await, EventKind::Cancelled));
    assert!(service.sessions().unwrap()[0].active_run.is_none());
    assert!(receiver.recv().now_or_never().is_none());
}

struct Mutation(std::sync::atomic::AtomicUsize);
#[async_trait::async_trait]
impl crate::tools::Tool for Mutation {
    fn name(&self) -> &str {
        "mutate"
    }
    fn description(&self) -> &str {
        "A test mutation"
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{},"additionalProperties":false})
    }
    fn is_read_only(&self) -> bool {
        false
    }
    async fn run(&self, _: Value, _: &crate::tools::ToolContext) -> crate::tools::ToolOutput {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        crate::tools::ToolOutput::ok("done")
    }
}

fn tool_sse() -> String {
    format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        json!({"id":"r1","object":"chat.completion.chunk","created":1,"model":"fixture","choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call1","type":"function","function":{"name":"mutate","arguments":"{}"}}]},"finish_reason":null}]}),
        json!({"id":"r1","object":"chat.completion.chunk","created":1,"model":"fixture","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]})
    )
}

#[tokio::test]
async fn mutations_wait_for_approval_and_denials_do_not_execute() {
    for approved in [true, false] {
        let (url, _) = fixture(vec![tool_sse(), text_sse("finished")]).await;
        let directory = tempfile::tempdir().unwrap();
        let service = AgentService::open(directory.path()).unwrap();
        let session = service.create_session().unwrap().id;
        let mut receiver = service.subscribe();
        let tool = Arc::new(Mutation(std::sync::atomic::AtomicUsize::new(0)));
        let mut input = request(session, url, "test approval");
        input.mode = Mode::Agent;
        input.tools.push(tool.clone());
        service.start(input).unwrap();
        let approval = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let event = receiver.recv().await.unwrap();
                if let EventKind::ApprovalRequested { id, .. } = event.kind {
                    break id;
                }
                assert!(
                    !event.kind.is_terminal(),
                    "unexpected ending: {:?}",
                    event.kind
                );
            }
        })
        .await
        .unwrap();
        assert_eq!(tool.0.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(service.approve(approval, approved));
        assert!(!service.approve(approval, true));
        assert!(matches!(ending(&mut receiver).await, EventKind::Completed));
        assert_eq!(
            tool.0.load(std::sync::atomic::Ordering::SeqCst),
            usize::from(approved)
        );
    }
}

#[tokio::test]
async fn ask_filters_mutations_even_if_the_host_supplied_them() {
    let (url, requests) = fixture(vec![text_sse("read only")]).await;
    let directory = tempfile::tempdir().unwrap();
    let service = AgentService::open(directory.path()).unwrap();
    let session = service.create_session().unwrap().id;
    let mut receiver = service.subscribe();
    let mut input = request(session, url, "inspect");
    input
        .tools
        .push(Arc::new(Mutation(std::sync::atomic::AtomicUsize::new(0))));
    service.start(input).unwrap();
    assert!(matches!(ending(&mut receiver).await, EventKind::Completed));
    assert!(!requests.lock().unwrap()[0].to_string().contains("mutate"));
}
