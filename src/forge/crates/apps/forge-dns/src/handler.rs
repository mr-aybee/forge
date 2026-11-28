//! The request handler: answers the zones from the current map and forwards
//! everything else.

use std::net::SocketAddr;
use std::sync::Arc;

use hickory_proto::op::{Edns, Header, HeaderCounts, MessageType, Metadata, OpCode, ResponseCode};
use hickory_proto::rr::Record;
use hickory_server::net::runtime::Time;
use hickory_server::server::{Request, RequestHandler, ResponseHandler, ResponseInfo};
use hickory_server::zone_handler::MessageResponseBuilder;

use crate::forward::forward;
use crate::watch::Current;
use crate::zone::Answer;

/// The EDNS version this server speaks.
const EDNS_VERSION: u8 = 0;

/// Answers queries for one server.
#[derive(Debug)]
pub struct Handler {
    pub current: Arc<Current>,
    pub upstreams: Arc<[SocketAddr]>,
}

/// The sections of a response.
#[derive(Debug, Default)]
struct Sections {
    code: ResponseCode,
    authoritative: bool,
    recursion_available: bool,
    answers: Vec<Record>,
    authorities: Vec<Record>,
    additionals: Vec<Record>,
}

impl Sections {
    fn code(code: ResponseCode) -> Self {
        Self { code, ..Self::default() }
    }
}

#[async_trait::async_trait]
impl RequestHandler for Handler {
    async fn handle_request<R: ResponseHandler, T: Time>(&self, request: &Request, mut response: R) -> ResponseInfo {
        // Respect the client's EDNS buffer size; 512 bytes without EDNS.
        let edns = request.edns.as_ref().map(|req| {
            let mut e = Edns::new();
            e.set_max_payload(req.max_payload().max(512));
            e.set_version(EDNS_VERSION);
            e
        });
        let sections = self.resolve(request).await;
        let mut builder = MessageResponseBuilder::from_message_request(request);

        if let Some(e) = &edns {
            builder.edns(e);
        }

        let mut metadata = Metadata::response_from_request(&request.metadata);
        metadata.response_code = sections.code;
        metadata.authoritative = sections.authoritative;
        metadata.recursion_available = sections.recursion_available;

        let message = builder.build(metadata, &sections.answers, &sections.authorities, &[], &sections.additionals);

        match response.send_response(message).await {
            Ok(info) => info,
            Err(e) => {
                tracing::debug!(src = %request.src(), error = %e, "sending response");

                let mut metadata = Metadata::new(request.metadata.id, MessageType::Response, request.metadata.op_code);
                metadata.response_code = ResponseCode::ServFail;

                Header { metadata, counts: HeaderCounts::default() }.into()
            }
        }
    }
}

impl Handler {
    async fn resolve(&self, request: &Request) -> Sections {
        if request.metadata.message_type != MessageType::Query || request.metadata.op_code != OpCode::Query {
            return Sections::code(ResponseCode::NotImp);
        }

        if request.edns.as_ref().is_some_and(|e| e.version() > EDNS_VERSION) {
            return Sections::code(ResponseCode::BADVERS);
        }

        let Ok(info) = request.request_info() else {
            return Sections::code(ResponseCode::FormErr);
        };

        // No valid file yet: the zones are unknown, so refuse everything
        // rather than leak cluster names to the upstreams.
        let Some(zone) = self.current.get() else {
            return Sections::code(ResponseCode::Refused);
        };

        let query = info.query.original();

        match zone.answer(query.name(), query.query_type()) {
            Some(Answer::Records { answers, soa }) => {
                Sections { authoritative: true, answers, authorities: soa.into_iter().collect(), ..Sections::default() }
            }
            Some(Answer::NxDomain { soa }) => Sections {
                code: ResponseCode::NXDomain,
                authoritative: true,
                authorities: vec![soa],
                ..Sections::default()
            },
            None => self.forward(request).await,
        }
    }

    async fn forward(&self, request: &Request) -> Sections {
        let Some(reply) = forward(&self.upstreams, request.as_slice()).await else {
            return Sections::code(ResponseCode::ServFail);
        };

        Sections {
            code: reply.metadata.response_code,
            authoritative: reply.metadata.authoritative,
            recursion_available: reply.metadata.recursion_available,
            answers: reply.answers,
            authorities: reply.authorities,
            additionals: reply.additionals,
        }
    }
}
