//! Model transport through Rig's OpenAI and Anthropic protocol adapters.
use crate::provider::Protocol;
use rig_core::{
    catalog::{ModelFacts, ModelSpec},
    driver::{Exchange, Model, Opening, Transport},
    error::ProviderError,
    http_client::DynHttpClient,
    operation::Completion,
    providers::{
        anthropic::AnthropicConfig,
        openai::{OpenAIConfig, responses_api::wire::Responses, wire},
    },
    serve::adapters::ModelAdapter,
    wire::{Encoded, Wire, WireFrame},
};
use std::time::Duration;

pub(crate) struct Backend {
    pub max_tokens: u64,
    pub supports_tools: bool,
    protocol: Protocol,
    url: String,
    secret: Option<String>,
    spec: ModelSpec,
    http: DynHttpClient,
}

impl Backend {
    pub fn new(
        protocol: Protocol,
        url: String,
        secret: Option<String>,
        spec: ModelSpec,
    ) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .build()
            .map_err(|error| error.to_string())?;
        let context = spec.context_window.unwrap_or(128_000);
        let max_tokens = 8192
            .min(context / 4)
            .min(spec.max_output_tokens.unwrap_or(8192))
            .max(1);
        Ok(Self {
            max_tokens: u64::from(max_tokens),
            supports_tools: spec.tools,
            protocol,
            url,
            secret,
            spec,
            http: DynHttpClient::new(rig_reqwest::ReqwestClient::from(client)),
        })
    }
    pub fn is_responses(&self) -> bool {
        self.protocol == Protocol::OpenaiResponses
    }

    pub fn adapter(self) -> ModelAdapter<Completion> {
        let model_id = self.spec.id.clone();
        let facts = ModelFacts::new(self.spec);
        let url = self.url.trim_end_matches('/');
        let model = if self.protocol == Protocol::AnthropicMessages {
            let config = AnthropicConfig::new(self.secret.unwrap_or_default()).with_base_url(url);
            let transport = MessagesRoute {
                http: self.http,
                url: format!("{url}/messages"),
            };
            Model::new(config.completion(&model_id).with_facts(facts), transport)
                .with_idle_timeout(Some(Duration::from_secs(120)))
                .erase()
        } else {
            let mut config =
                OpenAIConfig::with_key(&wire::OPENAI, self.secret.clone().unwrap_or_default())
                    .with_base_url(url);
            if self.secret.is_none() {
                config.auth = wire::Auth::OptionalBearer;
            }
            if self.protocol == Protocol::OpenaiResponses {
                let mut wire = Responses::new(config, &model_id).with_facts(facts);
                wire.strict_tools = false;
                Model::new(wire, self.http)
                    .with_idle_timeout(Some(Duration::from_secs(120)))
                    .erase()
            } else {
                Model::new(config.chat(&model_id).with_facts(facts), self.http)
                    .with_idle_timeout(Some(Duration::from_secs(120)))
                    .erase()
            }
        };
        ModelAdapter::new("default", model)
    }
}

// Keep the configured API root verbatim, including roots used by gateways.
#[derive(Clone)]
struct MessagesRoute {
    http: DynHttpClient,
    url: String,
}
impl<W: Wire<Payload = Encoded, Frame = WireFrame>> Transport<W> for MessagesRoute {
    fn send(&self, mut payload: Encoded, exchange: Exchange) -> Opening<WireFrame> {
        match self.url.parse() {
            Ok(uri) => *payload.request.uri_mut() = uri,
            Err(error) => {
                return Opening::failed(ProviderError::request(format!(
                    "invalid Messages URL: {error}"
                )));
            }
        }
        <DynHttpClient as Transport<W>>::send(&self.http, payload, exchange)
    }
}
